//! Site-wide crawl.
//!
//! Every page-level check in this binary answers "is this URL healthy?".
//! Real sites fail in ways only visible across pages — the same title on 400
//! product variants, a section nothing links to, a redirect chain that
//! survives because each hop looks fine alone. This walks the site and
//! aggregates.
//!
//! Deliberately conservative: same host by default, robots.txt honoured,
//! a hard page budget, and a small delay knob. A crawler shipped inside an
//! agent skill will be pointed at other people's sites, so the defaults are
//! the polite ones.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::ExitCode;
use std::sync::Mutex;

use serde_json::{json, Value};
use url::Url;

use crate::cmd::core::fetch_record;
use crate::html;
use crate::output::{err, print_json, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

/// Below this, a page is unlikely to answer anything on its own. The number
/// matches the thin-content threshold the content skills already use.
const THIN_WORDS: usize = 300;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Page {
    pub url: String,
    pub depth: usize,
    pub status: u16,
    pub title: Option<String>,
    pub meta_description: Option<String>,
    pub canonical: Option<String>,
    pub noindex: bool,
    pub h1_count: usize,
    pub word_count: usize,
    pub images: usize,
    pub images_missing_alt: usize,
    pub internal_links: usize,
    pub external_links: usize,
    pub has_schema: bool,
    pub bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Frontier {
    queue: VecDeque<(String, usize)>,
    seen: BTreeSet<String>,
    /// Where each URL was first linked from, so a broken link report can name
    /// the page that has to be edited.
    referrers: BTreeMap<String, String>,
    fetched: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    seed: &str,
    max_pages: usize,
    max_depth: usize,
    concurrency: usize,
    include_subdomains: bool,
    ignore_robots: bool,
    delay_ms: u64,
    timeout: u64,
    json: bool,
) -> CmdResult<ExitCode> {
    let start = crate::safety::coerce_scheme(seed);
    let parsed =
        Url::parse(&start).map_err(|e| crate::output::Error(format!("bad URL {seed}: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| crate::output::Error(format!("{seed} has no host")))?
        .to_ascii_lowercase();
    let apex = host.trim_start_matches("www.").to_string();

    let disallowed = if ignore_robots {
        Vec::new()
    } else {
        robots_disallow(&parsed)
    };

    let frontier = Mutex::new(Frontier {
        queue: VecDeque::from(vec![(start.clone(), 0usize)]),
        seen: BTreeSet::from([normalize(&start)]),
        referrers: BTreeMap::new(),
        fetched: 0,
    });
    let pages: Mutex<Vec<Page>> = Mutex::new(Vec::new());

    let workers = concurrency.clamp(1, 16);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                // Take one URL, or stop when the queue is empty *and* the
                // budget is not yet spent — an empty queue can be temporary
                // while another worker is still parsing links, so a worker
                // that finds nothing exits and the remaining ones drain it.
                let next = {
                    let mut f = frontier.lock().unwrap_or_else(|e| e.into_inner());
                    if f.fetched >= max_pages {
                        return;
                    }
                    match f.queue.pop_front() {
                        Some(item) => {
                            f.fetched += 1;
                            item
                        }
                        None => return,
                    }
                };
                let (url, depth) = next;

                if delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
                let rec = fetch_record(&url, timeout, true, None);
                let status = rec.status_code.unwrap_or(0);
                let body = rec.content.clone().unwrap_or_default();

                // A redirect means the page that came back is not the URL that
                // was queued. Claim the destination too, or the seed's own
                // `www.` hop leaves every later link to it looking unseen and
                // the page is fetched twice.
                let landed = normalize(&rec.url);
                if landed != normalize(&url) {
                    let already = {
                        let mut f = frontier.lock().unwrap_or_else(|e| e.into_inner());
                        !f.seen.insert(landed)
                    };
                    if already {
                        continue;
                    }
                }

                let page = if let Some(e) = rec.error.clone() {
                    Page {
                        url: url.clone(),
                        depth,
                        status,
                        title: None,
                        meta_description: None,
                        canonical: None,
                        noindex: false,
                        h1_count: 0,
                        word_count: 0,
                        images: 0,
                        images_missing_alt: 0,
                        internal_links: 0,
                        external_links: 0,
                        has_schema: false,
                        bytes: rec.bytes,
                        error: Some(e),
                    }
                } else {
                    let p = html::parse(&body, Some(&rec.url));
                    let robots_meta = p
                        .meta_robots
                        .clone()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let page = Page {
                        url: rec.url.clone(),
                        depth,
                        status,
                        title: p.title.clone(),
                        meta_description: p.meta_description.clone(),
                        canonical: p.canonical.clone(),
                        noindex: robots_meta.contains("noindex"),
                        h1_count: p.h1.len(),
                        word_count: p.word_count,
                        images: p.images.len(),
                        images_missing_alt: p
                            .images
                            .iter()
                            .filter(|i| i.alt.as_deref().unwrap_or("").trim().is_empty())
                            .count(),
                        internal_links: p.links.internal.len(),
                        external_links: p.links.external.len(),
                        has_schema: !p.schema.is_empty(),
                        bytes: rec.bytes,
                        error: None,
                    };

                    // Only follow links from pages that actually rendered, and
                    // only while there is depth left.
                    if status < 400 && depth < max_depth {
                        let mut f = frontier.lock().unwrap_or_else(|e| e.into_inner());
                        for link in &p.links.internal {
                            if f.seen.len() >= max_pages * 4 {
                                break;
                            }
                            let Some(abs) = absolutize(&rec.url, &link.href) else {
                                continue;
                            };
                            if !same_site(&abs, &apex, include_subdomains) {
                                continue;
                            }
                            if !ignore_robots && is_disallowed(&abs, &disallowed) {
                                continue;
                            }
                            let key = normalize(&abs);
                            if f.seen.insert(key) {
                                f.referrers
                                    .entry(abs.clone())
                                    .or_insert_with(|| rec.url.clone());
                                f.queue.push_back((abs, depth + 1));
                            }
                        }
                    }
                    page
                };

                if !json {
                    eprintln!("  [{status}] {}", page.url);
                }
                pages.lock().unwrap_or_else(|e| e.into_inner()).push(page);
            });
        }
    });

    let mut pages = pages.into_inner().unwrap_or_else(|e| e.into_inner());
    pages.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.url.cmp(&b.url)));
    if pages.is_empty() {
        return err(format!("crawled nothing from {seed}"));
    }
    let referrers = frontier
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .referrers;

    let report = aggregate(&pages, &referrers, &host);
    let doc = json!({
        "seed": start,
        "host": host,
        "crawled": pages.len(),
        "max_pages": max_pages,
        "max_depth": max_depth,
        "robots_respected": !ignore_robots,
        "summary": report,
        "pages": pages,
    });

    if json {
        print_json(&doc)?;
    } else {
        print_human(&doc);
    }
    if report["issues"]
        .as_array()
        .map(|a| a.iter().any(|i| i["severity"] == "critical"))
        .unwrap_or(false)
    {
        return Ok(ExitCode::from(2));
    }
    OK
}

/// Disallow rules that apply to this crawler.
///
/// `*` and the tool's own token are both honoured. A site that blocks `*`
/// but allows Googlebot is telling this tool to stay out, and it does.
fn robots_disallow(seed: &Url) -> Vec<String> {
    let Ok(robots_url) = seed.join("/robots.txt") else {
        return Vec::new();
    };
    let rec = fetch_record(robots_url.as_str(), 15, true, None);
    let Some(body) = rec.content else {
        return Vec::new();
    };
    if rec.status_code.unwrap_or(0) >= 400 {
        return Vec::new();
    }
    let (rules, _) = crate::cmd::core::parse_robots(&body);
    let mut out = Vec::new();
    for agent in ["*", "seogeo"] {
        if let Some(group) = rules.get(agent) {
            for (directive, path) in group {
                if directive == "disallow" && !path.is_empty() {
                    out.push(path.clone());
                }
            }
        }
    }
    out
}

fn is_disallowed(url: &str, rules: &[String]) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    let path = u.path();
    rules.iter().any(|r| {
        // robots.txt prefix matching, with `*` treated as "matches anything"
        // up to the next literal segment — enough for the patterns in use.
        match r.split_once('*') {
            Some((head, tail)) => {
                path.starts_with(head)
                    && (tail.is_empty() || path.contains(tail.trim_end_matches('$')))
            }
            None => path.starts_with(r.as_str()),
        }
    })
}

fn absolutize(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    for scheme in ["mailto:", "tel:", "javascript:", "data:", "sms:"] {
        if lower.starts_with(scheme) {
            return None;
        }
    }
    let mut abs = Url::parse(base).ok()?.join(href).ok()?;
    abs.set_fragment(None);
    if !matches!(abs.scheme(), "http" | "https") {
        return None;
    }
    Some(abs.to_string())
}

fn same_site(url: &str, apex: &str, include_subdomains: bool) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let bare = host.trim_start_matches("www.");
    if include_subdomains {
        bare == apex || bare.ends_with(&format!(".{apex}"))
    } else {
        bare == apex
    }
}

/// Dedupe key: trailing slash and fragment folded away, query kept because
/// `?page=2` is a different page.
fn normalize(url: &str) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_string();
    };
    u.set_fragment(None);
    let s = u.to_string();
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        s
    } else {
        trimmed.to_string()
    }
}

fn aggregate(pages: &[Page], referrers: &BTreeMap<String, String>, host: &str) -> Value {
    let ok: Vec<&Page> = pages
        .iter()
        .filter(|p| p.status > 0 && p.status < 400)
        .collect();

    let mut titles: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut descriptions: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in &ok {
        if let Some(t) = p.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            titles.entry(t).or_default().push(&p.url);
        }
        if let Some(d) = p
            .meta_description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            descriptions.entry(d).or_default().push(&p.url);
        }
    }
    let dup_titles: Vec<Value> = titles
        .iter()
        .filter(|(_, urls)| urls.len() > 1)
        .map(|(t, urls)| json!({"title": t, "count": urls.len(), "urls": urls.iter().take(5).collect::<Vec<_>>()}))
        .collect();
    let dup_descriptions: Vec<Value> = descriptions
        .iter()
        .filter(|(_, urls)| urls.len() > 1)
        .map(
            |(d, urls)| json!({"description": crate::output::truncate(d, 80), "count": urls.len()}),
        )
        .collect();

    let broken: Vec<Value> = pages
        .iter()
        .filter(|p| p.status >= 400 || p.error.is_some())
        .map(|p| {
            json!({
                "url": p.url,
                "status": p.status,
                "error": p.error,
                "linked_from": referrers.get(&p.url),
            })
        })
        .collect();

    let missing_title: Vec<&str> = ok
        .iter()
        .filter(|p| p.title.as_deref().unwrap_or("").trim().is_empty())
        .map(|p| p.url.as_str())
        .collect();
    let missing_desc: Vec<&str> = ok
        .iter()
        .filter(|p| {
            p.meta_description
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        })
        .map(|p| p.url.as_str())
        .collect();
    let thin: Vec<Value> = ok
        .iter()
        .filter(|p| p.word_count < THIN_WORDS)
        .map(|p| json!({"url": p.url, "words": p.word_count}))
        .collect();
    let no_h1: Vec<&str> = ok
        .iter()
        .filter(|p| p.h1_count == 0)
        .map(|p| p.url.as_str())
        .collect();
    let multi_h1: Vec<&str> = ok
        .iter()
        .filter(|p| p.h1_count > 1)
        .map(|p| p.url.as_str())
        .collect();
    let noindex: Vec<&str> = ok
        .iter()
        .filter(|p| p.noindex)
        .map(|p| p.url.as_str())
        .collect();
    let no_schema: Vec<&str> = ok
        .iter()
        .filter(|p| !p.has_schema)
        .map(|p| p.url.as_str())
        .collect();
    let no_canonical: Vec<&str> = ok
        .iter()
        .filter(|p| p.canonical.as_deref().unwrap_or("").trim().is_empty())
        .map(|p| p.url.as_str())
        .collect();
    let alt_gaps: usize = ok.iter().map(|p| p.images_missing_alt).sum();
    let total_images: usize = ok.iter().map(|p| p.images).sum();

    let mut issues = Vec::new();
    let mut add = |severity: &str, key: &str, count: usize, why: &str, sample: Value| {
        if count > 0 {
            issues.push(json!({
                "severity": severity,
                "issue": key,
                "count": count,
                "why": why,
                "sample": sample,
            }));
        }
    };
    add(
        "critical",
        "broken-internal-links",
        broken.len(),
        "A link that 404s costs the crawl budget spent on it and strands whatever it pointed at.",
        json!(broken.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "critical",
        "missing-title",
        missing_title.len(),
        "No title means no snippet in search and nothing for an answer engine to attribute.",
        json!(missing_title.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "warning",
        "duplicate-titles",
        dup_titles.len(),
        "Identical titles across pages force the engine to pick one and ignore the rest.",
        json!(dup_titles.iter().take(5).collect::<Vec<_>>()),
    );
    add(
        "warning",
        "missing-meta-description",
        missing_desc.len(),
        "The engine writes its own, from whatever text it finds first.",
        json!(missing_desc.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "warning",
        "duplicate-meta-descriptions",
        dup_descriptions.len(),
        "Usually a template that never got per-page values.",
        json!(dup_descriptions.iter().take(5).collect::<Vec<_>>()),
    );
    add(
        "warning",
        "thin-content",
        thin.len(),
        "Under 300 words rarely contains a self-contained answer worth quoting.",
        json!(thin.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "warning",
        "no-h1",
        no_h1.len(),
        "The H1 is the strongest single hint about what a page answers.",
        json!(no_h1.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "info",
        "multiple-h1",
        multi_h1.len(),
        "Valid HTML, but it splits the page's stated topic.",
        json!(multi_h1.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "info",
        "noindex",
        noindex.len(),
        "Intentional on utility pages; a problem when it lands on content.",
        json!(noindex.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "info",
        "no-structured-data",
        no_schema.len(),
        "Schema is how a page states its own facts rather than leaving them to be inferred.",
        json!(no_schema.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "info",
        "no-canonical",
        no_canonical.len(),
        "Leaves duplicate-URL resolution to the engine's guess.",
        json!(no_canonical.iter().take(10).collect::<Vec<_>>()),
    );
    add(
        "info",
        "images-missing-alt",
        alt_gaps,
        "Alt text is the only description of an image an answer engine can read.",
        json!({"images_total": total_images}),
    );

    let words: usize = ok.iter().map(|p| p.word_count).sum();
    json!({
        "pages_ok": ok.len(),
        "pages_failed": pages.len() - ok.len(),
        "host": host,
        "avg_word_count": if ok.is_empty() { 0 } else { words / ok.len() },
        "pages_with_schema": ok.iter().filter(|p| p.has_schema).count(),
        "max_depth_reached": pages.iter().map(|p| p.depth).max().unwrap_or(0),
        "issues": issues,
    })
}

fn print_human(doc: &Value) {
    let s = &doc["summary"];
    println!(
        "\ncrawled {} pages of {} — {} ok, {} failed, avg {} words",
        doc["crawled"].as_u64().unwrap_or(0),
        doc["host"].as_str().unwrap_or(""),
        s["pages_ok"].as_u64().unwrap_or(0),
        s["pages_failed"].as_u64().unwrap_or(0),
        s["avg_word_count"].as_u64().unwrap_or(0),
    );
    println!();
    for i in s["issues"].as_array().unwrap_or(&vec![]) {
        println!(
            "[{:<8}] {:<28} {:>5}   {}",
            i["severity"].as_str().unwrap_or(""),
            i["issue"].as_str().unwrap_or(""),
            i["count"].as_u64().unwrap_or(0),
            i["why"].as_str().unwrap_or(""),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_to_the_same_page_is_not_a_duplicate() {
        // The seed is usually written without a trailing slash and comes back
        // with one; that must not be mistaken for a redirect to a page that
        // was already crawled.
        assert_eq!(normalize("https://a.test"), normalize("https://a.test/"));
        assert_ne!(
            normalize("https://www.a.test/"),
            normalize("https://a.test/")
        );
    }

    #[test]
    fn normalize_folds_trailing_slash_and_fragment() {
        assert_eq!(
            normalize("https://a.test/x/"),
            normalize("https://a.test/x#y")
        );
    }

    #[test]
    fn same_site_respects_subdomain_flag() {
        assert!(same_site("https://www.a.test/x", "a.test", false));
        assert!(!same_site("https://blog.a.test/x", "a.test", false));
        assert!(same_site("https://blog.a.test/x", "a.test", true));
        assert!(!same_site("https://b.test/x", "a.test", true));
    }

    #[test]
    fn absolutize_skips_non_navigational_schemes() {
        assert!(absolutize("https://a.test/", "mailto:x@y.z").is_none());
        assert!(absolutize("https://a.test/", "#top").is_none());
        assert_eq!(
            absolutize("https://a.test/dir/page", "../other").as_deref(),
            Some("https://a.test/other")
        );
    }

    #[test]
    fn disallow_matches_prefix_and_wildcard() {
        let rules = vec!["/admin".to_string(), "/*.pdf$".to_string()];
        assert!(is_disallowed("https://a.test/admin/x", &rules));
        assert!(is_disallowed("https://a.test/files/spec.pdf", &rules));
        assert!(!is_disallowed("https://a.test/blog", &rules));
    }
}

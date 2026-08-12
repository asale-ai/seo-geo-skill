//! Core page-acquisition and parsing commands.

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde::Serialize;
use serde_json::json;
use url::Url;

use crate::chrome;
use crate::cli::{LlmsTxtAction, RenderMode};
use crate::html;
use crate::http::{self, RequestOptions, AI_CRAWLER_TOKENS, GOOGLEBOT_USER_AGENT};
use crate::output::{err, print_json, CmdResult, Error};
use crate::safety::{coerce_scheme, validate_url, validate_url_strict};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

fn fail() -> CmdResult<ExitCode> {
    Ok(ExitCode::from(1))
}

// --------------------------------------------------------------- url-safety

pub fn url_safety(url: &str, strict: bool, json: bool) -> CmdResult<ExitCode> {
    let mut result = json!({
        "url": url,
        "mode": if strict { "strict" } else { "parse" },
        "ok": false,
        "pinned_ip": serde_json::Value::Null,
        "error": serde_json::Value::Null,
    });

    if strict {
        match validate_url_strict(url) {
            Ok((_, ip)) => {
                result["ok"] = json!(true);
                result["pinned_ip"] = json!(ip.to_string());
            }
            Err(e) => result["error"] = json!(e.to_string()),
        }
    } else {
        result["ok"] = json!(validate_url(url));
    }

    let ok = result["ok"].as_bool().unwrap_or(false);
    if json {
        print_json(&result)?;
    } else if ok {
        let extra = result["pinned_ip"]
            .as_str()
            .map(|ip| format!(" -> {ip}"))
            .unwrap_or_default();
        println!("OK: {url}{extra}");
    } else {
        let reason = result["error"].as_str().unwrap_or("parse-time reject");
        println!("BLOCKED: {url} ({reason})");
    }
    if ok {
        OK
    } else {
        Ok(ExitCode::from(2))
    }
}

// -------------------------------------------------------------------- fetch

#[derive(Serialize)]
pub struct FetchRecord {
    pub url: String,
    pub status_code: Option<u16>,
    pub headers: BTreeMap<String, String>,
    pub content: Option<String>,
    pub error: Option<String>,
    pub bytes: usize,
    pub rendered: bool,
    /// Presence/value of the headers a technical audit reports on. `null`
    /// means the header was absent, which is the finding.
    pub security_headers: BTreeMap<String, Option<String>>,
}

fn security_header_summary(headers: &BTreeMap<String, String>) -> BTreeMap<String, Option<String>> {
    html::SECURITY_HEADERS
        .iter()
        .map(|h| (h.to_string(), headers.get(*h).cloned()))
        .collect()
}

/// Fetch a page. Returns the record even on transport failure so callers can
/// report the reason rather than an empty body.
pub fn fetch_record(url: &str, timeout: u64, follow: bool, ua: Option<&str>) -> FetchRecord {
    let normalized = coerce_scheme(url);
    let mut opts = RequestOptions::with_timeout(timeout);
    opts.follow_redirects = follow;
    if let Some(ua) = ua {
        opts.user_agent = Some(ua.to_string());
    }

    match http::get(&normalized, &opts) {
        Ok(resp) => {
            let text = resp.text();
            FetchRecord {
                url: resp.url.clone(),
                status_code: Some(resp.status),
                security_headers: security_header_summary(&resp.headers),
                headers: resp.headers.clone(),
                bytes: resp.body.len(),
                content: Some(text),
                error: None,
                rendered: false,
            }
        }
        Err(e) => FetchRecord {
            url: normalized,
            status_code: None,
            headers: BTreeMap::new(),
            security_headers: BTreeMap::new(),
            content: None,
            error: Some(e.to_string()),
            bytes: 0,
            rendered: false,
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn fetch(
    url: &str,
    output: Option<&str>,
    timeout: u64,
    follow: bool,
    user_agent: Option<&str>,
    googlebot: bool,
    render_mode: RenderMode,
    json: bool,
) -> CmdResult<ExitCode> {
    let ua = if googlebot {
        Some(GOOGLEBOT_USER_AGENT)
    } else {
        user_agent
    };

    let mut record = if render_mode == RenderMode::Never {
        fetch_record(url, timeout, follow, ua)
    } else {
        let rendered = render_page(url, render_mode, timeout * 1000, ua)?;
        FetchRecord {
            url: rendered.url,
            status_code: rendered.status_code,
            headers: BTreeMap::new(),
            security_headers: BTreeMap::new(),
            bytes: rendered.content.as_deref().map(str::len).unwrap_or(0),
            content: rendered.content,
            error: rendered.error,
            rendered: rendered.mode_used == "rendered",
        }
    };

    if let Some(e) = &record.error {
        eprintln!("Error: {e}");
        if json {
            print_json(&record)?;
        }
        return fail();
    }

    let body = record.content.take().unwrap_or_default();

    if let Some(path) = output {
        std::fs::write(path, &body)?;
        eprintln!("Saved to {path} ({} bytes)", body.len());
    }

    if json {
        record.content = if output.is_some() { None } else { Some(body) };
        print_json(&record)?;
    } else if output.is_none() {
        println!("{body}");
    }

    eprintln!("\nURL: {}", record.url);
    eprintln!("Status: {}", record.status_code.unwrap_or(0));
    OK
}

// -------------------------------------------------------------------- parse

pub fn parse(file: Option<&str>, url: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (source, base) = match (file, url) {
        (Some(path), _) => (std::fs::read_to_string(path)?, url.map(|u| u.to_string())),
        (None, Some(u)) => {
            let rec = fetch_record(u, 30, true, None);
            if let Some(e) = rec.error {
                return err(e);
            }
            (rec.content.unwrap_or_default(), Some(rec.url))
        }
        (None, None) => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            (buf, None)
        }
    };

    let parsed = html::parse(&source, base.as_deref());
    if json {
        print_json(&parsed)?;
    } else {
        println!("Title: {}", parsed.title.as_deref().unwrap_or("(none)"));
        println!(
            "Meta Description: {}",
            parsed.meta_description.as_deref().unwrap_or("(none)")
        );
        println!("Canonical: {}", parsed.canonical.as_deref().unwrap_or("(none)"));
        println!("H1 Tags: {}", parsed.h1.len());
        println!("H2 Tags: {}", parsed.h2.len());
        println!("Images: {}", parsed.images.len());
        println!("Internal Links: {}", parsed.links.internal.len());
        println!("External Links: {}", parsed.links.external.len());
        println!("Schema Blocks: {}", parsed.schema.len());
        println!("Word Count: {}", parsed.word_count);
    }
    OK
}

// ------------------------------------------------------------------- render

#[derive(Serialize)]
pub struct RenderRecord {
    pub url: String,
    pub status_code: Option<u16>,
    pub content: Option<String>,
    pub mode_used: String,
    pub is_spa: bool,
    pub raw_word_count: usize,
    pub rendered_word_count: Option<usize>,
    pub a11y_tree: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Decide whether a raw response is an SPA shell worth re-fetching through a
/// browser: a framework root with almost no text, or a page whose visible
/// word count is far below what its script payload suggests.
fn looks_like_spa(raw_html: &str, word_count: usize) -> bool {
    let (has_ssr, _) = html::ssr_assessment(raw_html, word_count);
    if !has_ssr {
        return true;
    }
    word_count < 100 && raw_html.matches("<script").count() >= 3
}

pub fn render_page(
    url: &str,
    mode: RenderMode,
    timeout_ms: u64,
    ua: Option<&str>,
) -> CmdResult<RenderRecord> {
    let normalized = coerce_scheme(url);
    let raw = fetch_record(&normalized, (timeout_ms / 1000).max(5), true, ua);
    if let Some(e) = &raw.error {
        if mode != RenderMode::Always {
            return Ok(RenderRecord {
                url: normalized,
                status_code: None,
                content: None,
                mode_used: "raw".into(),
                is_spa: false,
                raw_word_count: 0,
                rendered_word_count: None,
                a11y_tree: None,
                error: Some(e.clone()),
            });
        }
    }

    let raw_html = raw.content.clone().unwrap_or_default();
    let raw_words = html::word_count(&html::visible_text(&raw_html));
    let is_spa = looks_like_spa(&raw_html, raw_words);

    let should_render = match mode {
        RenderMode::Always => true,
        RenderMode::Auto => is_spa,
        RenderMode::Never => false,
    };

    if !should_render {
        return Ok(RenderRecord {
            url: raw.url,
            status_code: raw.status_code,
            content: Some(raw_html),
            mode_used: "raw".into(),
            is_spa,
            raw_word_count: raw_words,
            rendered_word_count: None,
            a11y_tree: None,
            error: raw.error,
        });
    }

    let dom = chrome::dump_dom(&normalized, timeout_ms, ua)?;
    let rendered_words = html::word_count(&html::visible_text(&dom));
    Ok(RenderRecord {
        url: raw.url,
        status_code: raw.status_code,
        content: Some(dom),
        mode_used: "rendered".into(),
        is_spa,
        raw_word_count: raw_words,
        rendered_word_count: Some(rendered_words),
        a11y_tree: None,
        error: None,
    })
}

pub fn render(
    url: &str,
    mode: RenderMode,
    timeout_ms: u64,
    a11y_tree: bool,
    ua: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let mut record = render_page(url, mode, timeout_ms, ua)?;
    if let Some(e) = &record.error {
        eprintln!("Error: {e}");
        return fail();
    }

    if a11y_tree {
        let content = record.content.clone().unwrap_or_default();
        record.a11y_tree = Some(accessibility_tree(&content));
    }

    if json {
        print_json(&record)?;
    } else {
        println!("{}", record.content.clone().unwrap_or_default());
        eprintln!(
            "\nURL: {} | mode={} | is_spa={} | raw_words={} | rendered_words={}",
            record.url,
            record.mode_used,
            record.is_spa,
            record.raw_word_count,
            record
                .rendered_word_count
                .map(|w| w.to_string())
                .unwrap_or_else(|| "-".into())
        );
    }
    OK
}

/// A landmark/heading outline — the structure a screen reader or an agent
/// traverses. Cheap stand-in for Chrome's full accessibility tree, and the
/// part an SEO audit actually reasons about.
fn accessibility_tree(html_src: &str) -> serde_json::Value {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html_src);
    let mut landmarks = Vec::new();
    let sel = Selector::parse(
        "main, nav, header, footer, aside, section, article, form, \
         [role=main], [role=navigation], [role=banner], [role=contentinfo], \
         [role=complementary], [role=search], [role=form]",
    )
    .unwrap();
    for el in doc.select(&sel) {
        let v = el.value();
        landmarks.push(json!({
            "tag": v.name(),
            "role": v.attr("role"),
            "label": v.attr("aria-label").or_else(|| v.attr("aria-labelledby")),
            "id": v.attr("id"),
        }));
    }

    let mut headings = Vec::new();
    let hsel = Selector::parse("h1, h2, h3, h4, h5, h6").unwrap();
    for el in doc.select(&hsel) {
        let text = el.text().collect::<String>().trim().to_string();
        if text.is_empty() {
            continue;
        }
        headings.push(json!({
            "level": el.value().name()[1..].parse::<u8>().unwrap_or(0),
            "text": crate::output::truncate(&text, 120),
        }));
    }

    let mut landmark_issues = Vec::new();
    if !html_src.contains("<main") && !html_src.contains("role=\"main\"") {
        landmark_issues.push("no <main> landmark — agents cannot isolate primary content");
    }
    let h1_count = headings.iter().filter(|h| h["level"] == 1).count();
    if h1_count == 0 {
        landmark_issues.push("no H1 — no top-level topic signal");
    } else if h1_count > 1 {
        landmark_issues.push("multiple H1s — ambiguous topic signal");
    }

    json!({
        "landmarks": landmarks,
        "headings": headings,
        "issues": landmark_issues,
    })
}

// ------------------------------------------------------- sitemap discovery

const COMMON_SITEMAP_PATHS: &[&str] = &[
    "/sitemap.xml",
    "/sitemap_index.xml",
    "/sitemap-index.xml",
    "/wp-sitemap.xml",
];
const MAX_DECLARED: usize = 16;
const MAX_ROBOTS_BYTES: usize = 1024 * 1024;
const MAX_SITEMAP_BYTES: usize = 50 * 1024 * 1024;

fn origin_of(input: &str) -> CmdResult<String> {
    let url = coerce_scheme(input);
    if !validate_url(&url) {
        return err("Target must be a public HTTP or HTTPS URL");
    }
    let parsed = Url::parse(&url).map_err(|e| Error(e.to_string()))?;
    let host = parsed.host_str().unwrap_or_default();
    let port = parsed
        .port()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    Ok(format!("{}://{host}{port}", parsed.scheme()))
}

/// Classify a fetched body as a sitemap. Returns `(kind, error)`.
fn sitemap_kind(body: &[u8], content_type: &str, url: &str) -> (Option<String>, Option<String>) {
    let trimmed: Vec<u8> = body
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .collect();
    if trimmed.is_empty() {
        return (None, Some("empty response".into()));
    }
    let upper = String::from_utf8_lossy(&trimmed[..trimmed.len().min(4096)]).to_uppercase();
    if upper.contains("<!DOCTYPE") {
        return (None, Some("DOCTYPE is not allowed in sitemap XML".into()));
    }

    if trimmed.starts_with(b"<") {
        return match xml_root_local_name(&trimmed) {
            Some(local) => match local.as_str() {
                "urlset" | "sitemapindex" | "rss" | "feed" => (Some(local), None),
                _ => (None, Some("unsupported XML root".into())),
            },
            None => (None, Some("invalid XML".into())),
        };
    }

    let is_text = content_type.to_ascii_lowercase().contains("text/plain")
        || url.to_ascii_lowercase().ends_with(".txt");
    if is_text {
        let text = String::from_utf8_lossy(&trimmed);
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.len() > 50_000 {
            return (
                None,
                Some("text sitemap exceeds the 50,000 URL protocol limit".into()),
            );
        }
        // Syntax-only check: resolving up to 50k hosts here would be an
        // unbounded DNS fan-out. Connection-time validation still applies to
        // every candidate discovery actually fetches.
        let all_valid = !lines.is_empty()
            && lines.iter().all(|l| {
                Url::parse(l)
                    .map(|u| {
                        matches!(u.scheme(), "http" | "https")
                            && u.host_str().is_some()
                            && u.username().is_empty()
                            && u.password().is_none()
                    })
                    .unwrap_or(false)
            });
        if all_valid {
            return (Some("text".into()), None);
        }
        return (None, Some("invalid text sitemap".into()));
    }
    (None, Some("response is not a supported sitemap format".into()))
}

fn xml_root_local_name(body: &[u8]) -> Option<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name();
                return Some(String::from_utf8_lossy(name.as_ref()).to_ascii_lowercase());
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => buf.clear(),
        }
    }
}

/// Strip userinfo, query, and fragment so a discovered URL is safe to print.
fn display_url(raw: &str) -> (String, bool) {
    match Url::parse(raw) {
        Ok(u) => {
            let host = u.host_str().unwrap_or_default();
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let path = if u.path().is_empty() { "/" } else { u.path() };
            let redacted =
                !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some();
            (format!("{}://{host}{port}{path}", u.scheme()), redacted)
        }
        Err(_) => (raw.to_string(), true),
    }
}

pub fn sitemap_discovery(url: &str, json: bool) -> CmdResult<ExitCode> {
    let origin = match origin_of(url) {
        Ok(o) => o,
        Err(e) => {
            let out = json!({"target": null, "error": e.to_string()});
            if json {
                print_json(&out)?;
            } else {
                eprintln!("Error: {e}");
            }
            return fail();
        }
    };

    let robots_url = format!("{origin}/robots.txt");
    let mut warnings: Vec<String> = Vec::new();
    let mut declared_raw: Vec<String> = Vec::new();

    let robots_opts = RequestOptions::with_timeout(30).max_bytes(MAX_ROBOTS_BYTES);
    match http::get(&robots_url, &robots_opts) {
        Ok(r) if r.status == 200 => {
            for line in r.text().lines() {
                let lower = line.trim().to_ascii_lowercase();
                if let Some(rest) = lower.strip_prefix("sitemap:") {
                    let idx = line.to_ascii_lowercase().find("sitemap:").unwrap() + "sitemap:".len();
                    let value = line[idx..].trim().to_string();
                    let _ = rest;
                    if !value.is_empty() && !declared_raw.contains(&value) {
                        declared_raw.push(value);
                    }
                }
            }
        }
        Ok(r) => warnings.push(format!("robots.txt returned HTTP {}", r.status)),
        Err(_) => warnings.push("robots.txt could not be fetched safely".into()),
    }

    if declared_raw.len() > MAX_DECLARED {
        warnings.push(format!(
            "robots.txt declared more than {MAX_DECLARED} sitemaps; extra entries were not fetched"
        ));
        declared_raw.truncate(MAX_DECLARED);
    }

    let declared: Vec<serde_json::Value> = declared_raw
        .iter()
        .map(|d| {
            let (disp, redacted) = display_url(d);
            json!({"url": disp, "query_redacted": redacted})
        })
        .collect();

    let mut candidates: Vec<String> = declared_raw.clone();
    for p in COMMON_SITEMAP_PATHS {
        candidates.push(format!("{origin}{p}"));
    }
    candidates.dedup();

    let target_host = Url::parse(&origin)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let mut checked = Vec::new();
    let mut found = Vec::new();
    let opts = RequestOptions::with_timeout(30).max_bytes(MAX_SITEMAP_BYTES);

    for candidate in &candidates {
        let (disp, query_redacted) = display_url(candidate);
        let source = if declared_raw.contains(candidate) {
            "robots.txt"
        } else {
            "common_path"
        };
        let mut entry = json!({
            "url": disp,
            "query_redacted": query_redacted,
            "source": source,
            "status_code": serde_json::Value::Null,
            "kind": serde_json::Value::Null,
            "valid": false,
            "error": serde_json::Value::Null,
        });
        if source == "robots.txt" {
            let host = Url::parse(candidate).ok().and_then(|u| u.host_str().map(String::from));
            if host.as_deref() != Some(target_host.as_str()) {
                entry["cross_host"] = json!(true);
            }
        }

        match http::get(candidate, &opts) {
            Err(e) => entry["error"] = json!(e.to_string()),
            Ok(resp) => {
                entry["status_code"] = json!(resp.status);
                if !(200..300).contains(&resp.status) {
                    entry["error"] = json!(format!("HTTP {}", resp.status));
                } else {
                    let ct = resp.header("content-type").unwrap_or_default().to_string();
                    let (kind, kerr) = sitemap_kind(&resp.body, &ct, &resp.url);
                    entry["kind"] = json!(kind);
                    entry["error"] = json!(kerr);
                    entry["valid"] = json!(kind.is_some());
                    if kind.is_some() {
                        let (final_disp, final_redacted) = display_url(&resp.url);
                        entry["url"] = json!(final_disp);
                        entry["query_redacted"] = json!(query_redacted || final_redacted);
                        found.push(entry.clone());
                    }
                }
            }
        }
        checked.push(entry);
    }

    let result = json!({
        "target": origin,
        "robots_url": robots_url,
        "declared": declared,
        "found": found,
        "checked": checked,
        "warnings": warnings,
        "error": serde_json::Value::Null,
    });

    if json {
        print_json(&result)?;
    } else if result["found"].as_array().is_some_and(|a| !a.is_empty()) {
        for item in result["found"].as_array().unwrap() {
            println!(
                "{} ({})",
                item["url"].as_str().unwrap_or_default(),
                item["kind"].as_str().unwrap_or("?")
            );
        }
    } else {
        println!("No valid sitemap found");
    }
    OK
}

// ------------------------------------------------------------------ robots

pub fn robots(url: &str, json: bool) -> CmdResult<ExitCode> {
    let origin = origin_of(url)?;
    let robots_url = format!("{origin}/robots.txt");
    let mut result = json!({
        "url": robots_url,
        "exists": false,
        "content": "",
        "ai_crawler_status": {},
        "sitemaps": [],
        "errors": [],
    });

    let opts = RequestOptions::with_timeout(15).max_bytes(MAX_ROBOTS_BYTES);
    match http::get(&robots_url, &opts) {
        Ok(resp) if resp.status == 200 => {
            let text = resp.text();
            result["exists"] = json!(true);
            result["content"] = json!(text.clone());
            let (rules, sitemaps) = parse_robots(&text);
            result["sitemaps"] = json!(sitemaps);

            let mut status = serde_json::Map::new();
            for crawler in AI_CRAWLER_TOKENS {
                let key = crawler.to_ascii_lowercase();
                let verdict = if let Some(directives) = rules.get(&key) {
                    if directives
                        .iter()
                        .any(|(d, p)| d == "disallow" && p == "/")
                    {
                        "BLOCKED"
                    } else if directives.iter().any(|(d, p)| d == "disallow" && !p.is_empty()) {
                        "PARTIALLY_BLOCKED"
                    } else {
                        "ALLOWED"
                    }
                } else if let Some(wildcard) = rules.get("*") {
                    if wildcard.iter().any(|(d, p)| d == "disallow" && p == "/") {
                        "BLOCKED_BY_WILDCARD"
                    } else {
                        "ALLOWED_BY_DEFAULT"
                    }
                } else {
                    "NOT_MENTIONED"
                };
                status.insert(crawler.to_string(), json!(verdict));
            }
            result["ai_crawler_status"] = serde_json::Value::Object(status);
        }
        Ok(resp) if resp.status == 404 => {
            result["errors"] = json!(["No robots.txt found (404)"]);
            let mut status = serde_json::Map::new();
            for crawler in AI_CRAWLER_TOKENS {
                status.insert(crawler.to_string(), json!("NO_ROBOTS_TXT"));
            }
            result["ai_crawler_status"] = serde_json::Value::Object(status);
        }
        Ok(resp) => {
            result["errors"] = json!([format!("Unexpected status code: {}", resp.status)]);
        }
        Err(e) => {
            result["errors"] = json!([format!("Error fetching robots.txt: {e}")]);
        }
    }

    // robots.txt is a request, not a guarantee: servers and WAFs frequently
    // block AI user agents outright. Probe the origin with each major crawler
    // UA so "ALLOWED" in robots.txt is not mistaken for "reachable".
    let mut live = serde_json::Map::new();
    for (name, ua) in crate::http::AI_CRAWLERS {
        let mut opts = RequestOptions::with_timeout(12);
        opts.user_agent = Some((*ua).to_string());
        opts.max_bytes = 32 * 1024;
        let probe = match http::get(&origin, &opts) {
            Ok(r) => json!({
                "status_code": r.status,
                "served": (200..400).contains(&r.status),
                "error": serde_json::Value::Null,
            }),
            Err(e) => json!({"status_code": null, "served": false, "error": e.to_string()}),
        };
        live.insert((*name).to_string(), probe);
    }
    result["live_crawler_fetch"] = serde_json::Value::Object(live);

    if json {
        print_json(&result)?;
    } else {
        println!("robots.txt: {}", result["url"].as_str().unwrap_or_default());
        println!("Exists: {}", result["exists"]);
        if let Some(map) = result["ai_crawler_status"].as_object() {
            for (k, v) in map {
                println!("  {k:<22} {}", v.as_str().unwrap_or("?"));
            }
        }
        if let Some(sitemaps) = result["sitemaps"].as_array() {
            for s in sitemaps {
                println!("  sitemap: {}", s.as_str().unwrap_or_default());
            }
        }
        println!("Live fetch as each crawler UA:");
        for (name, probe) in result["live_crawler_fetch"].as_object().unwrap() {
            println!(
                "  {name:<16} served={} status={}",
                probe["served"], probe["status_code"]
            );
        }
    }
    OK
}

type RobotsRules = BTreeMap<String, Vec<(String, String)>>;

/// Parse robots.txt into per-agent directives plus declared sitemaps.
/// Consecutive `User-agent` lines share the record that follows them, which
/// is what the RFC specifies and what a naive line-by-line parser gets wrong.
pub fn parse_robots(text: &str) -> (RobotsRules, Vec<String>) {
    let mut rules: RobotsRules = BTreeMap::new();
    let mut sitemaps = Vec::new();
    let mut current_agents: Vec<String> = Vec::new();
    let mut expecting_agents = false;

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let field = field.trim().to_ascii_lowercase();
        let value = value.trim().to_string();

        match field.as_str() {
            "user-agent" => {
                if !expecting_agents {
                    current_agents.clear();
                    expecting_agents = true;
                }
                let agent = value.to_ascii_lowercase();
                rules.entry(agent.clone()).or_default();
                current_agents.push(agent);
            }
            "disallow" | "allow" => {
                expecting_agents = false;
                for agent in &current_agents {
                    rules
                        .entry(agent.clone())
                        .or_default()
                        .push((field.clone(), value.clone()));
                }
            }
            "sitemap" => {
                // "Sitemap: https://..." splits on the first colon, so the
                // scheme lands in the value only if we re-join it.
                let full = if value.starts_with("http") {
                    value.clone()
                } else {
                    let idx = line.to_ascii_lowercase().find("sitemap:").unwrap() + "sitemap:".len();
                    line[idx..].trim().to_string()
                };
                if !full.is_empty() && !sitemaps.contains(&full) {
                    sitemaps.push(full);
                }
            }
            _ => {}
        }
    }
    (rules, sitemaps)
}

// ----------------------------------------------------------------- llms.txt

pub fn llms_txt(action: LlmsTxtAction) -> CmdResult<ExitCode> {
    match action {
        LlmsTxtAction::Validate { url, json } => llms_validate(&url, json),
        LlmsTxtAction::Generate {
            url,
            max_pages,
            output,
            json,
        } => llms_generate(&url, max_pages, output.as_deref(), json),
    }
}

fn llms_validate(url: &str, json: bool) -> CmdResult<ExitCode> {
    let origin = origin_of(url)?;
    let llms_url = format!("{origin}/llms.txt");
    let llms_full_url = format!("{origin}/llms-full.txt");

    let mut issues: Vec<String> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();
    let mut exists = false;
    let mut content = String::new();
    let (mut has_title, mut has_description, mut has_sections, mut has_links) =
        (false, false, false, false);
    let (mut section_count, mut link_count) = (0usize, 0usize);

    let opts = RequestOptions::with_timeout(15);
    match http::get(&llms_url, &opts) {
        Ok(resp) if resp.status == 200 => {
            exists = true;
            content = resp.text();
            let lines: Vec<&str> = content.trim().lines().collect();

            if lines.first().is_some_and(|l| l.starts_with("# ")) {
                has_title = true;
            } else {
                issues.push("Missing title (should start with '# Site Name')".into());
            }
            has_description = lines.iter().any(|l| l.starts_with("> "));
            if !has_description {
                issues.push("Missing description (use '> Brief description')".into());
            }
            section_count = lines.iter().filter(|l| l.starts_with("## ")).count();
            has_sections = section_count > 0;
            if !has_sections {
                issues.push("No sections found (use '## Section Name')".into());
            }
            let link_re = regex::Regex::new(r"(?m)^\s*-\s*\[.+\]\(.+\)").unwrap();
            link_count = link_re.find_iter(&content).count();
            has_links = link_count > 0;
            if !has_links {
                issues.push(
                    "No page links found (use '- [Page Title](url): Description')".into(),
                );
            }

            if link_count < 5 {
                suggestions.push("Consider adding more key pages (aim for 10-20)".into());
            }
            if section_count < 2 {
                suggestions.push("Add more sections to organize content types".into());
            }
            let lower = content.to_ascii_lowercase();
            if !lower.contains("contact") {
                suggestions.push("Add a Contact section with email and location".into());
            }
            if !lower.contains("key fact") && !lower.contains("about") {
                suggestions.push("Add key facts about your business/service".into());
            }
        }
        Ok(resp) => issues.push(format!("llms.txt returned status {}", resp.status)),
        Err(e) => issues.push(format!("Error fetching llms.txt: {e}")),
    }

    // Only the status matters here, and llms-full.txt is routinely tens of
    // megabytes, so cap the read rather than downloading the whole file.
    let probe = RequestOptions::with_timeout(15).max_bytes(4096);
    let full_exists = http::get(&llms_full_url, &probe)
        .map(|r| r.status == 200)
        .unwrap_or(false);

    let format_valid = has_title && has_description && has_sections && has_links;
    let result = json!({
        "url": llms_url,
        "exists": exists,
        "format_valid": format_valid,
        "has_title": has_title,
        "has_description": has_description,
        "has_sections": has_sections,
        "has_links": has_links,
        "section_count": section_count,
        "link_count": link_count,
        "content": content,
        "issues": issues,
        "suggestions": suggestions,
        "full_version": {"url": llms_full_url, "exists": full_exists},
    });

    if json {
        print_json(&result)?;
    } else {
        println!("llms.txt: {llms_url}");
        println!("  exists:       {exists}");
        println!("  format_valid: {format_valid}");
        println!("  sections:     {section_count}");
        println!("  links:        {link_count}");
        println!("  llms-full.txt: {full_exists}");
        for i in result["issues"].as_array().unwrap() {
            println!("  issue: {}", i.as_str().unwrap_or_default());
        }
    }
    if exists && format_valid {
        OK
    } else {
        fail()
    }
}

const LLMS_SECTIONS: &[(&str, &[&str])] = &[
    ("Products & Services", &["/pricing", "/feature", "/product", "/solution", "/demo"]),
    (
        "Resources & Blog",
        &["/blog", "/article", "/resource", "/guide", "/learn", "/docs", "/documentation"],
    ),
    ("Company", &["/about", "/team", "/career", "/contact", "/press", "/partner"]),
    ("Support", &["/help", "/support", "/faq", "/status"]),
];

fn llms_generate(
    url: &str,
    max_pages: usize,
    output: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let normalized = coerce_scheme(url);
    let origin = origin_of(&normalized)?;
    let rec = fetch_record(&normalized, 30, true, None);
    if let Some(e) = rec.error {
        return err(format!("Failed to fetch homepage: {e}"));
    }
    let body = rec.content.unwrap_or_default();
    let parsed = html::parse(&body, Some(&rec.url));

    let site_name = parsed
        .title
        .as_deref()
        .map(|t| {
            t.split(['|', '-', '—', '·'])
                .next()
                .unwrap_or(t)
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Url::parse(&origin)
                .ok()
                .and_then(|u| u.host_str().map(String::from))
                .unwrap_or_default()
        });
    let host = Url::parse(&origin)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    let site_description = parsed
        .meta_description
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("Official website of {site_name}"));

    let mut buckets: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();

    for link in &parsed.links.internal {
        if seen.len() >= max_pages {
            break;
        }
        let text = link.text.trim();
        if text.chars().count() < 2 {
            continue;
        }
        let Ok(u) = Url::parse(&link.href) else { continue };
        let clean = format!(
            "{}://{}{}",
            u.scheme(),
            u.host_str().unwrap_or_default(),
            u.path()
        );
        if seen.contains(&clean) {
            continue;
        }
        let path = u.path().to_ascii_lowercase();
        if [".pdf", ".jpg", ".jpeg", ".png", ".gif", ".svg", ".css", ".js"]
            .iter()
            .any(|ext| path.ends_with(ext))
        {
            continue;
        }
        seen.push(clean.clone());

        let mut section = "Main Pages";
        for (name, keywords) in LLMS_SECTIONS {
            if keywords.iter().any(|k| path.contains(k)) {
                section = name;
                break;
            }
        }
        if (path == "/" || path.is_empty()) && (clean == origin || clean == format!("{origin}/")) {
            continue;
        }
        buckets.entry(section).or_default().push((text.to_string(), clean));
    }

    let order = [
        "Main Pages",
        "Products & Services",
        "Resources & Blog",
        "Company",
        "Support",
    ];

    let mut concise = vec![format!("# {site_name}"), format!("> {site_description}"), String::new()];
    let mut full = concise.clone();

    for section in order {
        let Some(pages) = buckets.get(section) else { continue };
        if pages.is_empty() {
            continue;
        }
        concise.push(format!("## {section}"));
        full.push(format!("## {section}"));
        for (title, href) in pages.iter().take(10) {
            concise.push(format!("- [{title}]({href})"));
        }
        concise.push(String::new());

        for (title, href) in pages {
            // Only same-origin pages get a description fetch; anything else
            // would turn generation into an open redirect follower.
            let desc = if href.starts_with(&origin) {
                let sub = fetch_record(href, 10, true, None);
                sub.content
                    .as_deref()
                    .and_then(|b| html::parse(b, Some(href)).meta_description)
                    .filter(|d| !d.trim().is_empty())
            } else {
                None
            };
            match desc {
                Some(d) => full.push(format!("- [{title}]({href}): {d}")),
                None => full.push(format!("- [{title}]({href})")),
            }
        }
        full.push(String::new());
    }

    for lines in [&mut concise, &mut full] {
        lines.push("## Contact".into());
        lines.push(format!("- Website: {origin}"));
        lines.push(format!("- Email: contact@{host}"));
        lines.push(String::new());
    }

    let generated = concise.join("\n");
    let generated_full = full.join("\n");
    let sections: BTreeMap<&str, usize> =
        buckets.iter().map(|(k, v)| (*k, v.len())).collect();

    if let Some(path) = output {
        std::fs::write(path, &generated)?;
        let full_path = if let Some(stripped) = path.strip_suffix(".txt") {
            format!("{stripped}-full.txt")
        } else {
            format!("{path}.full")
        };
        std::fs::write(&full_path, &generated_full)?;
        eprintln!("Wrote {path} and {full_path}");
    }

    let result = json!({
        "generated_llmstxt": generated,
        "generated_llmstxt_full": generated_full,
        "pages_analyzed": seen.len(),
        "sections": sections,
    });

    if json {
        print_json(&result)?;
    } else if output.is_none() {
        println!("{generated}");
    }
    OK
}

// ------------------------------------------------------------------ blocks

pub fn blocks(url: &str, min_words: usize, json: bool) -> CmdResult<ExitCode> {
    let rec = fetch_record(url, 30, true, None);
    if let Some(e) = rec.error {
        return err(e);
    }
    let body = rec.content.unwrap_or_default();
    let blocks = html::content_blocks(&body, min_words);
    if json {
        print_json(&blocks)?;
    } else {
        for b in &blocks {
            println!("[{}] {} words", b.heading, b.word_count);
        }
    }
    OK
}

// ----------------------------------------------------------- crawl sitemap

/// Walk a site's sitemaps (index files included) and collect page URLs.
pub fn collect_sitemap_urls(origin: &str, max_pages: usize) -> Vec<String> {
    let opts = RequestOptions::with_timeout(20).max_bytes(MAX_SITEMAP_BYTES);
    let mut queue: Vec<String> = COMMON_SITEMAP_PATHS
        .iter()
        .map(|p| format!("{origin}{p}"))
        .collect();

    // robots.txt declarations first — they are authoritative.
    if let Ok(resp) = http::get(&format!("{origin}/robots.txt"), &opts) {
        if resp.status == 200 {
            let (_, sitemaps) = parse_robots(&resp.text());
            for s in sitemaps.into_iter().rev() {
                queue.insert(0, s);
            }
        }
    }

    let mut pages: Vec<String> = Vec::new();
    let mut visited: Vec<String> = Vec::new();
    let mut depth = 0;

    while let Some(sitemap_url) = queue.first().cloned() {
        queue.remove(0);
        if visited.contains(&sitemap_url) || depth > 64 {
            continue;
        }
        visited.push(sitemap_url.clone());
        depth += 1;

        let Ok(resp) = http::get(&sitemap_url, &opts) else { continue };
        if resp.status != 200 {
            continue;
        }
        let (locs, is_index) = parse_sitemap_locs(&resp.body);
        if is_index {
            for loc in locs {
                if !visited.contains(&loc) {
                    queue.push(loc);
                }
            }
        } else {
            for loc in locs {
                if !pages.contains(&loc) {
                    pages.push(loc);
                }
                if pages.len() >= max_pages {
                    return pages;
                }
            }
        }
    }
    pages.truncate(max_pages);
    pages
}

/// Extract `<loc>` values. The second value is true for a `<sitemapindex>`.
fn parse_sitemap_locs(body: &[u8]) -> (Vec<String>, bool) {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut locs = Vec::new();
    let mut in_loc = false;
    let mut is_index = false;
    let mut root_seen = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                if !root_seen {
                    root_seen = true;
                    is_index = name == "sitemapindex";
                }
                if name == "loc" {
                    in_loc = true;
                }
            }
            Ok(Event::Text(t)) if in_loc => {
                if let Ok(s) = t.unescape() {
                    let v = s.trim().to_string();
                    if !v.is_empty() {
                        locs.push(v);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                if name == "loc" {
                    in_loc = false;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (locs, is_index)
}

pub fn crawl_sitemap(url: &str, max_pages: usize, json: bool) -> CmdResult<ExitCode> {
    let origin = origin_of(url)?;
    let pages = collect_sitemap_urls(&origin, max_pages);
    if json {
        print_json(&json!({"pages": pages, "count": pages.len()}))?;
    } else {
        for p in &pages {
            println!("{p}");
        }
        eprintln!("{} URLs", pages.len());
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_groups_consecutive_agents() {
        let txt = "User-agent: GPTBot\nUser-agent: ClaudeBot\nDisallow: /\n\n\
                   User-agent: *\nDisallow: /admin\nSitemap: https://x.test/sitemap.xml\n";
        let (rules, sitemaps) = parse_robots(txt);
        assert_eq!(
            rules.get("gptbot").unwrap(),
            &vec![("disallow".to_string(), "/".to_string())]
        );
        assert_eq!(
            rules.get("claudebot").unwrap(),
            &vec![("disallow".to_string(), "/".to_string())]
        );
        assert_eq!(sitemaps, vec!["https://x.test/sitemap.xml"]);
    }

    #[test]
    fn sitemap_index_detected() {
        let xml = br#"<?xml version="1.0"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <sitemap><loc>https://x.test/s1.xml</loc></sitemap></sitemapindex>"#;
        let (locs, is_index) = parse_sitemap_locs(xml);
        assert!(is_index);
        assert_eq!(locs, vec!["https://x.test/s1.xml"]);
    }

    #[test]
    fn urlset_detected() {
        let xml = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://x.test/a</loc></url><url><loc>https://x.test/b</loc></url></urlset>"#;
        let (locs, is_index) = parse_sitemap_locs(xml);
        assert!(!is_index);
        assert_eq!(locs.len(), 2);
    }

    #[test]
    fn doctype_rejected_as_sitemap() {
        let (kind, e) = sitemap_kind(b"<!DOCTYPE html><html></html>", "text/html", "https://x/s");
        assert!(kind.is_none());
        assert!(e.unwrap().contains("DOCTYPE"));
    }
}

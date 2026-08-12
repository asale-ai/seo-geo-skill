//! Backlink commands: Moz Link Explorer, Bing Webmaster Tools, the Common
//! Crawl host graph, and first-party link verification.
//!
//! Credentials live in `~/.config/seogeo/backlinks-api.json` with env
//! fallbacks. Verification needs no credentials at all — it fetches each
//! claimed linking page and checks whether the link is really there, which
//! is the only source of truth in a backlink report.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::ExitCode;

use base64::Engine;
use serde_json::{json, Value};
use url::Url;

use crate::cli::{BingAction, MozAction};
use crate::cmd::core::fetch_record;
use crate::http::{self, RequestOptions};
use crate::output::{err, print_json, truncate, CmdResult, Error};
use crate::safety::coerce_scheme;

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

const MOZ_BASE: &str = "https://api.moz.com";
const BING_BASE: &str = "https://ssl.bing.com/webmaster/api.svc/json";
const CC_GRAPH_BASE: &str = "https://data.commoncrawl.org/projects/hyperlinkgraph";

/// Newest first. Check https://commoncrawl.github.io/cc-webgraph-statistics/
/// when a release stops resolving.
const CC_RELEASES: &[&str] = &[
    "cc-main-2026-jan-feb-mar",
    "cc-main-2025-oct-nov-dec",
    "cc-main-2025-jul-aug-sep",
    "cc-main-2025-apr-may-jun",
    "cc-main-2025-jan-feb-mar",
    "cc-main-2024-oct-nov-dec",
];

pub fn config_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("backlinks-api.json")
}

#[derive(Default, Clone, Debug)]
pub struct BacklinkConfig {
    pub moz_api_key: Option<String>,
    pub bing_api_key: Option<String>,
}

pub fn load_config() -> BacklinkConfig {
    let mut cfg = BacklinkConfig::default();
    if let Ok(raw) = std::fs::read_to_string(config_path()) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            cfg.moz_api_key = v["moz_api_key"].as_str().map(String::from);
            cfg.bing_api_key = v["bing_api_key"].as_str().map(String::from);
        }
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    cfg.moz_api_key = cfg.moz_api_key.or_else(|| env("MOZ_API_KEY"));
    cfg.bing_api_key = cfg
        .bing_api_key
        .or_else(|| env("BING_WEBMASTER_API_KEY"))
        .or_else(|| env("BING_API_KEY"));
    cfg
}

pub fn auth(check: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let cfg = load_config();
    let wanted = check.unwrap_or("all");
    let mut providers = serde_json::Map::new();

    if wanted == "all" || wanted == "moz" {
        providers.insert(
            "moz".into(),
            json!({
                "service": "Moz Link Explorer v2",
                "configured": cfg.moz_api_key.is_some(),
                "env": "MOZ_API_KEY",
                "signup": "https://moz.com/products/api (free tier: 2,500 rows/month)",
            }),
        );
    }
    if wanted == "all" || wanted == "bing" {
        providers.insert(
            "bing".into(),
            json!({
                "service": "Bing Webmaster Tools",
                "configured": cfg.bing_api_key.is_some(),
                "env": "BING_WEBMASTER_API_KEY",
                "signup": "https://www.bing.com/webmasters (free)",
            }),
        );
    }
    if wanted == "all" || wanted == "commoncrawl" {
        providers.insert(
            "commoncrawl".into(),
            json!({
                "service": "Common Crawl host graph",
                "configured": true,
                "env": null,
                "signup": "no credentials required",
            }),
        );
    }
    if providers.is_empty() {
        return err(format!("unknown provider {wanted:?}"));
    }

    let out = json!({"config_path": config_path().display().to_string(), "providers": providers});
    let all_ok = out["providers"]
        .as_object()
        .unwrap()
        .values()
        .all(|v| v["configured"].as_bool().unwrap_or(false));

    if json {
        print_json(&out)?;
    } else {
        println!("Config: {}", config_path().display());
        for (k, v) in out["providers"].as_object().unwrap() {
            println!(
                "  [{}] {:<12} {}",
                if v["configured"].as_bool().unwrap_or(false) {
                    "OK "
                } else {
                    "-- "
                },
                k,
                v["service"].as_str().unwrap_or("")
            );
            if !v["configured"].as_bool().unwrap_or(false) {
                println!(
                    "       set {} — {}",
                    v["env"],
                    v["signup"].as_str().unwrap_or("")
                );
            }
        }
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// -------------------------------------------------------------------- Moz

/// Moz accepts either `id:secret` Basic auth or a bare token header.
fn moz_headers(api_key: &str, timeout: u64) -> RequestOptions {
    let opts = RequestOptions::with_timeout(timeout);
    if api_key.contains(':') {
        let encoded = base64::engine::general_purpose::STANDARD.encode(api_key);
        opts.header("Authorization", format!("Basic {encoded}"))
    } else {
        opts.header("x-moz-token", api_key)
    }
}

fn moz_request(path: &str, body: &Value, api_key: &str) -> CmdResult<Value> {
    let url = format!("{MOZ_BASE}{path}");
    let resp = http::post_json(&url, body, &moz_headers(api_key, 30))?;
    let text = resp.text();
    match resp.status {
        429 => err("Moz rate limit exceeded. Wait and verify your current plan limits."),
        401 => err("Invalid Moz API key. Check it at https://moz.com/products/api/keys"),
        403 => err("Moz API access denied. The free tier may not include this endpoint."),
        s if s >= 400 => {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v["error"]
                        .as_str()
                        .or_else(|| v["message"].as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| truncate(&text, 300));
            err(format!("HTTP {s}: {msg}"))
        }
        _ => Ok(serde_json::from_str(&text)?),
    }
}

/// Moz targets are bare hosts, not URLs.
fn moz_target(input: &str) -> String {
    input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

fn require_moz() -> CmdResult<String> {
    load_config().moz_api_key.ok_or_else(|| {
        Error(
            "Moz API key not configured. Set MOZ_API_KEY (free tier at \
             https://moz.com/products/api)."
                .into(),
        )
    })
}

pub fn moz(action: MozAction) -> CmdResult<ExitCode> {
    let key = require_moz()?;
    match action {
        MozAction::Metrics { target, json } => {
            let t = moz_target(&target);
            let raw = moz_request("/v2/url_metrics", &json!({"targets": [t]}), &key)?;
            let first = &raw["results"][0];
            let result = json!({
                "target": target,
                "domain_authority": first["domain_authority"],
                "page_authority": first["page_authority"],
                "spam_score": first["spam_score"],
                "linking_root_domains": first["root_domains_to_page"],
                "external_links": first["external_pages_to_page"],
                "last_crawled": first["last_crawled"],
                "raw": first,
            });
            if json {
                print_json(&result)?;
            } else {
                println!("Target: {target}");
                println!("  Domain Authority:  {}", result["domain_authority"]);
                println!("  Page Authority:    {}", result["page_authority"]);
                println!("  Spam Score:        {}", result["spam_score"]);
                println!("  Referring domains: {}", result["linking_root_domains"]);
            }
            OK
        }
        MozAction::Domains {
            target,
            limit,
            json,
        } => {
            let body = json!({
                "target": moz_target(&target),
                "target_scope": "root_domain",
                "limit": limit.min(50),
            });
            let raw = moz_request("/v2/linking_root_domains", &body, &key)?;
            let domains: Vec<Value> = raw["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|i| {
                    json!({
                        "domain": i["root_domain"],
                        "domain_authority": i["domain_authority"],
                        "spam_score": i["spam_score"],
                        "links_to_target": i["to_target"]["pages"],
                    })
                })
                .collect();
            let result = json!({"target": target, "count": domains.len(), "domains": domains});
            if json {
                print_json(&result)?;
            } else {
                for d in &domains {
                    println!(
                        "  {:<45} DA={:<5} spam={}",
                        truncate(d["domain"].as_str().unwrap_or(""), 45),
                        d["domain_authority"],
                        d["spam_score"]
                    );
                }
            }
            OK
        }
        MozAction::Anchors {
            target,
            limit,
            json,
        } => {
            let body = json!({
                "target": moz_target(&target),
                "target_scope": "root_domain",
                "limit": limit.min(50),
            });
            let raw = moz_request("/v2/anchor_text", &body, &key)?;
            let anchors: Vec<Value> = raw["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|i| {
                    json!({
                        "anchor_text": i["anchor_text"],
                        "external_links": i["external_pages"],
                        "linking_domains": i["external_root_domains"],
                    })
                })
                .collect();
            let result = json!({"target": target, "count": anchors.len(), "anchors": anchors});
            if json {
                print_json(&result)?;
            } else {
                for a in &anchors {
                    println!(
                        "  {:<45} links={} domains={}",
                        truncate(a["anchor_text"].as_str().unwrap_or(""), 45),
                        a["external_links"],
                        a["linking_domains"]
                    );
                }
            }
            OK
        }
        MozAction::Pages {
            target,
            limit,
            json,
        } => {
            let body = json!({
                "target": moz_target(&target),
                "target_scope": "root_domain",
                "limit": limit.min(50),
            });
            let raw = moz_request("/v2/top_pages", &body, &key)?;
            let pages: Vec<Value> = raw["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|i| {
                    json!({
                        "url": i["page"],
                        "page_authority": i["page_authority"],
                        "links": i["external_pages_to_page"],
                        "linking_domains": i["root_domains_to_page"],
                    })
                })
                .collect();
            let result = json!({"target": target, "count": pages.len(), "pages": pages});
            if json {
                print_json(&result)?;
            } else {
                for p in &pages {
                    println!(
                        "  {:<55} PA={}",
                        truncate(p["url"].as_str().unwrap_or(""), 55),
                        p["page_authority"]
                    );
                }
            }
            OK
        }
    }
}

// ------------------------------------------------------------------- Bing

fn require_bing() -> CmdResult<String> {
    load_config().bing_api_key.ok_or_else(|| {
        Error(
            "Bing Webmaster API key not configured. Set BING_WEBMASTER_API_KEY \
             (free at https://www.bing.com/webmasters)."
                .into(),
        )
    })
}

fn normalize_site(url: &str) -> String {
    let u = coerce_scheme(url);
    Url::parse(&u)
        .ok()
        .and_then(|p| p.host_str().map(|h| format!("{}://{}/", p.scheme(), h)))
        .unwrap_or(u)
}

fn bing_request(endpoint: &str, api_key: &str, params: &[(&str, String)]) -> CmdResult<Value> {
    let mut url = format!("{BING_BASE}/{endpoint}?apikey={api_key}");
    for (k, v) in params {
        url.push_str(&format!("&{k}={}", http::enc(v)));
    }
    let resp = http::get(&url, &RequestOptions::with_timeout(30))?;
    let text = resp.text();
    if !(200..300).contains(&resp.status) {
        // Never echo the URL back — it carries the API key.
        return err(format!(
            "Bing Webmaster {endpoint} returned HTTP {}: {}",
            resp.status,
            truncate(&text, 300)
        ));
    }
    Ok(serde_json::from_str(&text)?)
}

/// Bing wraps every payload in `{"d": ...}`.
fn bing_payload(v: &Value) -> Value {
    v.get("d").cloned().unwrap_or_else(|| v.clone())
}

fn bing_link_counts(site: &str, api_key: &str) -> CmdResult<Value> {
    let raw = bing_request(
        "GetLinkCounts",
        api_key,
        &[("siteUrl", site.to_string()), ("page", "0".into())],
    )?;
    Ok(bing_payload(&raw))
}

pub fn bing(action: BingAction) -> CmdResult<ExitCode> {
    let key = require_bing()?;
    match action {
        BingAction::Links { url, json } => {
            let site = normalize_site(&url);
            let counts = bing_link_counts(&site, &key)?;
            let entries: Vec<Value> = counts["Details"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|d| {
                    json!({
                        "url": d["Url"],
                        "inbound_links": d["Count"],
                    })
                })
                .collect();
            let total: i64 = entries
                .iter()
                .filter_map(|e| e["inbound_links"].as_i64())
                .sum();
            let result = json!({
                "site": site,
                "total_inbound_links": total,
                "pages": entries.len(),
                "details": entries,
                "note": "Bing Webmaster only reports link data for sites verified in your account.",
            });
            if json {
                print_json(&result)?;
            } else {
                println!(
                    "{site}: {total} inbound links across {} pages",
                    entries.len()
                );
                for e in entries.iter().take(30) {
                    println!(
                        "  {:<60} {}",
                        truncate(e["url"].as_str().unwrap_or(""), 60),
                        e["inbound_links"]
                    );
                }
            }
            OK
        }
        BingAction::Compare { url_a, url_b, json } => {
            let (a, b) = (normalize_site(&url_a), normalize_site(&url_b));
            let ca = bing_link_counts(&a, &key)?;
            let cb = bing_link_counts(&b, &key)?;
            let total = |c: &Value| -> i64 {
                c["Details"]
                    .as_array()
                    .map(|d| d.iter().filter_map(|x| x["Count"].as_i64()).sum())
                    .unwrap_or(0)
            };
            let (ta, tb) = (total(&ca), total(&cb));
            let result = json!({
                "a": {"site": a, "total_inbound_links": ta},
                "b": {"site": b, "total_inbound_links": tb},
                "gap": ta - tb,
                "leader": if ta >= tb { &a } else { &b },
            });
            if json {
                print_json(&result)?;
            } else {
                println!("{a}: {ta} inbound links");
                println!("{b}: {tb} inbound links");
                println!("gap: {}", ta - tb);
            }
            OK
        }
    }
}

// ----------------------------------------------------------- Common Crawl

/// Common Crawl stores hosts reversed (`com.example`), which is what makes
/// the sorted graph files scannable.
fn reverse_host(domain: &str) -> String {
    let host = Url::parse(&coerce_scheme(domain))
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| domain.trim().to_string());
    let host = host.trim_start_matches("www.");
    host.split('.').rev().collect::<Vec<_>>().join(".")
}

fn cc_file_url(release: &str, suffix: &str) -> String {
    format!("{CC_GRAPH_BASE}/{release}/domain/{release}{suffix}")
}

/// Stream a gzipped graph file and pull the lines mentioning the target.
///
/// These files are multi-gigabyte, so we decompress incrementally and stop
/// at the first match or at the byte cap — whichever comes first — and
/// report which happened rather than silently returning nothing.
fn scan_cc_file(
    url: &str,
    reversed: &str,
    max_compressed_bytes: usize,
) -> CmdResult<(Vec<Vec<String>>, bool)> {
    let resp = http::get(
        url,
        &RequestOptions::with_timeout(180).max_bytes(max_compressed_bytes),
    )?;
    if !(200..300).contains(&resp.status) {
        return err(format!("Common Crawl file returned HTTP {}", resp.status));
    }
    let capped = resp.body.len() >= max_compressed_bytes;

    let mut decoder = flate2::read::MultiGzDecoder::new(&resp.body[..]);
    let mut text = String::new();
    // A truncated gzip stream errors at the tail; keep whatever decoded.
    let _ = decoder.read_to_string(&mut text);

    let mut matches = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<String> = line.split('\t').map(String::from).collect();
        if fields
            .iter()
            .any(|f| f == reversed || f.ends_with(&format!(".{reversed}")))
        {
            matches.push(fields);
            if matches.len() >= 10 {
                break;
            }
        }
    }
    Ok((matches, capped))
}

pub fn commoncrawl(domain: &str, max_scan_mb: usize, json: bool) -> CmdResult<ExitCode> {
    let reversed = reverse_host(domain);
    let release = CC_RELEASES[0];
    let ranks_url = cc_file_url(release, "-domain-ranks.txt.gz");

    // The rank file is sorted by rank, so the head of it holds every
    // well-known domain. Scanning further costs minutes for diminishing
    // returns, which is why the cap is a flag rather than a constant.
    let (matches, capped) = scan_cc_file(&ranks_url, &reversed, max_scan_mb * 1024 * 1024)?;

    // Rank file layout: harmonic_pos, harmonic_val, pagerank_pos, pagerank_val, host
    let record = matches.first().map(|f| {
        json!({
            "harmonic_centrality_rank": f.first().and_then(|v| v.parse::<f64>().ok()),
            "harmonic_centrality_value": f.get(1).and_then(|v| v.parse::<f64>().ok()),
            "pagerank_rank": f.get(2).and_then(|v| v.parse::<f64>().ok()),
            "pagerank_value": f.get(3).and_then(|v| v.parse::<f64>().ok()),
            "host": f.get(4),
        })
    });

    let result = json!({
        "domain": domain,
        "reversed_host": reversed,
        "release": release,
        "source": ranks_url,
        "found": record.is_some(),
        "metrics": record,
        "truncated_scan": capped,
        "scanned_mb": max_scan_mb,
        "note": if capped && matches.is_empty() {
            "The scan hit its byte cap before finding the domain. Raise --max-scan-mb, or use \
             Moz or Bing data for domains outside the head of the graph."
        } else {
            "Common Crawl host-graph ranks are free and credential-free but update quarterly."
        },
        "known_releases": CC_RELEASES,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Domain: {domain}  (graph key: {reversed})");
        println!("Release: {release}");
        match &record {
            Some(m) => {
                println!(
                    "  harmonic centrality rank: {}",
                    m["harmonic_centrality_rank"]
                );
                println!("  pagerank rank:            {}", m["pagerank_rank"]);
            }
            None => println!("  not found in the scanned portion of the graph"),
        }
        if capped {
            println!("  (scan truncated at the byte cap)");
        }
    }
    OK
}

// ------------------------------------------------------ backlink verification

pub fn verify(target: &str, links_file: &str, timeout: u64, json: bool) -> CmdResult<ExitCode> {
    let target_url =
        Url::parse(&coerce_scheme(target)).map_err(|e| Error(format!("invalid --target: {e}")))?;
    let target_host = target_url.host_str().unwrap_or_default().to_string();
    let target_host_bare = target_host.trim_start_matches("www.").to_string();

    let candidates: Vec<String> = std::fs::read_to_string(links_file)
        .map_err(|e| Error(format!("could not read {links_file}: {e}")))?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    if candidates.is_empty() {
        return err(format!("no URLs found in {links_file}"));
    }

    let mut results = Vec::new();
    for candidate in &candidates {
        let rec = fetch_record(candidate, timeout, true, None);
        if let Some(e) = rec.error {
            results.push(json!({
                "source": candidate, "status": "unreachable", "error": e,
                "link_found": false, "followed": false,
            }));
            continue;
        }
        let body = rec.content.unwrap_or_default();
        let parsed = crate::html::parse(&body, Some(&rec.url));

        // A link counts only if it actually points at the target host.
        let hits: Vec<&crate::html::LinkInfo> = parsed
            .links
            .external
            .iter()
            .chain(parsed.links.internal.iter())
            .filter(|l| {
                Url::parse(&l.href)
                    .ok()
                    .and_then(|u| {
                        u.host_str()
                            .map(|h| h.trim_start_matches("www.").to_string())
                    })
                    .is_some_and(|h| h == target_host_bare)
            })
            .collect();

        let followed = hits
            .iter()
            .any(|l| !l.rel.iter().any(|r| r.eq_ignore_ascii_case("nofollow")));
        let has_sponsored_or_ugc = hits.iter().any(|l| {
            l.rel
                .iter()
                .any(|r| r.eq_ignore_ascii_case("sponsored") || r.eq_ignore_ascii_case("ugc"))
        });
        let noindex = parsed
            .meta_robots
            .as_deref()
            .is_some_and(|r| r.to_lowercase().contains("noindex"));

        results.push(json!({
            "source": rec.url,
            "status": rec.status_code,
            "link_found": !hits.is_empty(),
            "link_count": hits.len(),
            "followed": followed,
            "rel_sponsored_or_ugc": has_sponsored_or_ugc,
            "source_page_noindex": noindex,
            "anchors": hits.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
            "hrefs": hits.iter().map(|l| l.href.clone()).collect::<Vec<_>>(),
        }));
    }

    let found = results.iter().filter(|r| r["link_found"] == true).count();
    let followed = results.iter().filter(|r| r["followed"] == true).count();
    let result = json!({
        "target": target,
        "target_host": target_host,
        "checked": results.len(),
        "verified": found,
        "followed": followed,
        "nofollow_or_missing": results.len() - followed,
        "results": results,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Target: {target}");
        println!(
            "Checked {} link(s): {found} present, {followed} followed",
            results.len()
        );
        for r in &result["results"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>()
        {
            let mark = match (r["link_found"].as_bool(), r["followed"].as_bool()) {
                (Some(true), Some(true)) => "OK  ",
                (Some(true), _) => "NOFO",
                _ => "MISS",
            };
            println!(
                "  [{mark}] {}",
                truncate(r["source"].as_str().unwrap_or(""), 70)
            );
        }
    }
    Ok(if found == results.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// ---------------------------------------------------- report validation

/// Structural gate for a backlink report before it goes to a client: every
/// claimed link needs a source URL, a target, and verification evidence.
pub fn validate_report(file: &str, json: bool) -> CmdResult<ExitCode> {
    let raw =
        std::fs::read_to_string(file).map_err(|e| Error(format!("could not read {file}: {e}")))?;
    let data: Value =
        serde_json::from_str(&raw).map_err(|e| Error(format!("{file} is not valid JSON: {e}")))?;

    let mut issues: Vec<Value> = Vec::new();
    let mut push = |severity: &str, path: String, message: String| {
        issues.push(json!({"severity": severity, "path": path, "message": message}));
    };

    let links = data["links"]
        .as_array()
        .or_else(|| data["results"].as_array())
        .or_else(|| data.as_array());
    let Some(links) = links else {
        push(
            "error",
            "$".into(),
            "report has no `links` / `results` array and is not an array itself".into(),
        );
        let result = json!({"file": file, "valid": false, "issues": issues});
        print_json(&result)?;
        return Ok(ExitCode::from(1));
    };

    if links.is_empty() {
        push(
            "warning",
            "$.links".into(),
            "report contains no links".into(),
        );
    }

    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    for (i, link) in links.iter().enumerate() {
        let path = format!("$.links[{i}]");
        let source = link["source"]
            .as_str()
            .or_else(|| link["source_url"].as_str())
            .or_else(|| link["url"].as_str());
        match source {
            None => push("error", path.clone(), "no source URL".into()),
            Some(s) => {
                *sources.entry(s.to_string()).or_insert(0) += 1;
                if Url::parse(s).is_err() {
                    push(
                        "error",
                        path.clone(),
                        format!("source {s:?} is not a valid URL"),
                    );
                }
            }
        }
        if link["target"].is_null() && link["target_url"].is_null() {
            push("warning", path.clone(), "no target URL recorded".into());
        }
        let verified = link["link_found"]
            .as_bool()
            .or_else(|| link["verified"].as_bool());
        match verified {
            None => push(
                "error",
                path.clone(),
                "no verification evidence (`link_found` / `verified`)".into(),
            ),
            Some(false) => push(
                "warning",
                path.clone(),
                "link was not found on the source page".into(),
            ),
            Some(true) => {}
        }
        if link["followed"].is_null() && link["rel"].is_null() {
            push("warning", path, "no follow/nofollow status recorded".into());
        }
    }

    for (source, count) in &sources {
        if *count > 1 {
            push(
                "warning",
                "$.links".into(),
                format!("source {source} appears {count} times — deduplicate before reporting"),
            );
        }
    }

    let errors = issues.iter().filter(|i| i["severity"] == "error").count();
    let warnings = issues.iter().filter(|i| i["severity"] == "warning").count();
    let result = json!({
        "file": file,
        "link_count": links.len(),
        "unique_sources": sources.len(),
        "valid": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "issues": issues,
    });

    if json {
        print_json(&result)?;
    } else {
        println!(
            "{file}: {} link(s), {} unique source(s)",
            links.len(),
            sources.len()
        );
        println!("Errors: {errors}   Warnings: {warnings}");
        for i in result["issues"].as_array().unwrap() {
            println!(
                "  [{}] {} — {}",
                i["severity"].as_str().unwrap_or(""),
                i["path"].as_str().unwrap_or(""),
                i["message"].as_str().unwrap_or("")
            );
        }
    }
    Ok(if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverses_hosts_for_the_graph() {
        assert_eq!(reverse_host("https://www.example.com/x"), "com.example");
        assert_eq!(reverse_host("sub.example.co.uk"), "uk.co.example.sub");
    }

    #[test]
    fn moz_targets_strip_scheme_and_slash() {
        assert_eq!(moz_target("https://example.com/"), "example.com");
        assert_eq!(moz_target("example.com"), "example.com");
    }

    #[test]
    fn normalizes_bing_site_urls() {
        assert_eq!(normalize_site("example.com/path"), "https://example.com/");
    }
}

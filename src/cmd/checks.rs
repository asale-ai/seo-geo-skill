//! Technical audit checks that need no API credentials.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;

use regex::Regex;
use serde_json::json;
use url::Url;

use crate::cli::IptcAction;
use crate::cmd::core::fetch_record;
use crate::html;
use crate::http::{self, RequestOptions};
use crate::output::{err, print_json, CmdResult, Error};
use crate::safety::{coerce_scheme, validate_url_strict};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

fn code(pass: bool) -> CmdResult<ExitCode> {
    Ok(if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

// ------------------------------------------------------------ preload check

/// Audit the four mechanisms that move next-navigation paint cost off the
/// critical path. LCP is the binding Core Web Vitals constraint for most
/// sites, and speculation rules plus bfcache are the largest wins available
/// without a content rewrite.
pub fn preload(url: &str, json: bool) -> CmdResult<ExitCode> {
    let rec = fetch_record(url, 20, true, None);
    if let Some(e) = rec.error {
        return err(e);
    }
    let body = rec.content.unwrap_or_default();
    let headers = rec.headers;

    let spec_block = re(
        r#"(?is)<script\b[^>]*\btype\s*=\s*["']speculationrules["'][^>]*>(.*?)</script>"#,
    );
    let blocks: Vec<String> = spec_block
        .captures_iter(&body)
        .map(|c| c[1].to_string())
        .collect();
    let mut actions: Vec<String> = Vec::new();
    for b in &blocks {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(b.trim()) {
            for a in ["prefetch", "prerender"] {
                if v.get(a).and_then(|x| x.as_array()).is_some() && !actions.contains(&a.to_string())
                {
                    actions.push(a.to_string());
                }
            }
        }
    }
    actions.sort();
    let header_present = headers.contains_key("speculation-rules");

    let preload_count = re(r#"(?i)<link\b[^>]*\brel\s*=\s*["']preload["'][^>]*>"#)
        .find_iter(&body)
        .count();
    let prerender_count = re(r#"(?i)<link\b[^>]*\brel\s*=\s*["']prerender["'][^>]*>"#)
        .find_iter(&body)
        .count();
    let fetchpriority_count = re(r#"(?i)\bfetchpriority\s*=\s*["']high["']"#)
        .find_iter(&body)
        .count();
    let lcp_img_hint = re(
        r#"(?is)<(?:img|video|source)\b[^>]*\bfetchpriority\s*=\s*["']high["']"#,
    )
    .is_match(&body);

    let cc = headers.get("cache-control").cloned().unwrap_or_default();
    let cache_control_no_store = cc.to_ascii_lowercase().contains("no-store");
    let has_unload = re(r#"(?i)\b(?:addEventListener|on)\s*\(\s*["']?unload["']?"#).is_match(&body);
    let has_beforeunload =
        re(r#"(?i)\b(?:addEventListener|on)\s*\(\s*["']?beforeunload["']?"#).is_match(&body);

    let mut score = 0;
    let mut recs: Vec<String> = Vec::new();

    if !blocks.is_empty() || header_present {
        score += 25;
    } else {
        recs.push(
            "Add <script type=\"speculationrules\"> for prefetch+prerender on top user paths. \
             Saves the entire next-navigation paint cost."
                .into(),
        );
    }
    if lcp_img_hint {
        score += 25;
    } else {
        recs.push(
            "Mark the LCP hero image with fetchpriority=\"high\" so the browser preloads it \
             ahead of other resources."
                .into(),
        );
    }
    if !cache_control_no_store && !has_unload {
        score += 25;
    } else {
        if cache_control_no_store {
            recs.push(
                "Cache-Control: no-store disqualifies the page from bfcache. Remove it or scope \
                 it to authenticated routes only."
                    .into(),
            );
        }
        if has_unload {
            recs.push(
                "An unload listener disqualifies the page from bfcache. Switch to pagehide or \
                 visibilitychange."
                    .into(),
            );
        }
    }
    if prerender_count == 0 {
        score += 25;
    } else {
        recs.push(format!(
            "Found {prerender_count} <link rel=\"prerender\"> (deprecated). Migrate to \
             speculation rules."
        ));
    }

    let result = json!({
        "url": rec.url,
        "speculation_rules": {
            "inline_blocks": blocks.len(),
            "header_present": header_present,
            "actions": actions,
        },
        "preload_hints": preload_count,
        "prerender_links": prerender_count,
        "bfcache_signals": {
            "cache_control_no_store": cache_control_no_store,
            "unload_listener": has_unload,
            "beforeunload_listener": has_beforeunload,
        },
        "lcp_resource_hints": {
            "preload_lcp_candidate": lcp_img_hint,
            "fetchpriority_high": fetchpriority_count,
        },
        "score": score,
        "recommendations": recs,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {}", result["url"].as_str().unwrap_or_default());
        println!("Score: {score}/100");
        println!(
            "  Speculation Rules:    blocks={} header={header_present} actions={:?}",
            blocks.len(),
            result["speculation_rules"]["actions"]
        );
        println!("  Preload hints:        {preload_count}");
        println!("  Deprecated prerender: {prerender_count}");
        println!(
            "  bfcache killers:      no-store={cache_control_no_store} unload={has_unload} \
             beforeunload={has_beforeunload}"
        );
        println!(
            "  LCP preload:          marked={lcp_img_hint} fetchpriority=high count={fetchpriority_count}"
        );
        for r in result["recommendations"].as_array().unwrap() {
            println!("  - {}", r.as_str().unwrap_or_default());
        }
    }
    code(score >= 75)
}

// ----------------------------------------------------------- parasite risk

const THIRD_PARTY_PATTERN: &str = r"(?i)\bPartner\s+Content\b|\bSponsored\s+Content\b|\bSponsored\s+by\b|\bBrand\s+Studio\b|\bIn\s+Partnership\s+With\b|\bAdvertisement\b|\bAdvertorial\b|\bPaid\s+Post\b|\bPromoted\b|\bPaid\s+Content\b";
const COMMERCE_PATTERN: &str = r"(?i)\bBuy\s+Now\b|\bShop\s+Now\b|\bAdd\s+to\s+Cart\b|\bCompare\s+Prices\b|\bBest\s+\w+\s+Deals?\b|\bPromo\s+Code\b|\bCoupon\b|\bDiscount\s+Code\b|\bAffiliate\s+Disclosure\b";
const AFFILIATE_PATTERN: &str =
    r"(?i)\b(?:tag=|aff_id=|affid=|partnerid=|ref_=|utm_source=|utm_campaign=)";

fn subfolder(url: &str) -> String {
    Url::parse(url)
        .ok()
        .map(|u| {
            let first = u
                .path_segments()
                .and_then(|mut s| s.find(|p| !p.is_empty()).map(|p| p.to_string()));
            match first {
                Some(seg) => format!("/{seg}/"),
                None => "/".to_string(),
            }
        })
        .unwrap_or_else(|| "/".to_string())
}

pub fn parasite_risk(urls: &[String], urls_file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let mut targets: Vec<String> = urls.to_vec();
    if let Some(path) = urls_file {
        for line in std::fs::read_to_string(path)?.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                targets.push(line.to_string());
            }
        }
    }
    if targets.is_empty() {
        return err("pass URLs as positional args or via --urls-file");
    }

    let third = re(THIRD_PARTY_PATTERN);
    let commerce = re(COMMERCE_PATTERN);
    let affiliate = re(AFFILIATE_PATTERN);

    let mut rows: Vec<(String, usize, usize, usize)> = Vec::new();
    let mut errors: Vec<serde_json::Value> = Vec::new();

    for url in &targets {
        let rec = fetch_record(url, 20, true, None);
        match rec.error {
            Some(e) => errors.push(json!({"url": url, "error": e})),
            None => {
                let body = rec.content.unwrap_or_default();
                rows.push((
                    rec.url,
                    third.find_iter(&body).count(),
                    commerce.find_iter(&body).count(),
                    affiliate.find_iter(&body).count(),
                ));
            }
        }
    }

    let mut by_section: BTreeMap<String, Vec<&(String, usize, usize, usize)>> = BTreeMap::new();
    for row in &rows {
        by_section.entry(subfolder(&row.0)).or_default().push(row);
    }

    let mut sections = serde_json::Map::new();
    let mut rates: Vec<f64> = Vec::new();

    for (section, pages) in &by_section {
        let n = pages.len() as f64;
        let tp = pages.iter().map(|p| p.1).sum::<usize>() as f64 / n;
        let cm = pages.iter().map(|p| p.2).sum::<usize>() as f64 / n;
        let af = pages.iter().map(|p| p.3).sum::<usize>() as f64 / n;
        rates.push(cm);

        let mut flags: Vec<String> = Vec::new();
        if tp >= 1.0 {
            flags.push("third-party-authorship-density".into());
        }
        if cm >= 2.0 {
            flags.push("commercial-intent-skew".into());
        }
        if af >= 3.0 {
            flags.push("affiliate-density".into());
        }
        let has = |name: &str| flags.iter().any(|f| f == name);
        // Third-party authorship is high risk on its own; commercial skew and
        // affiliate density only together, because either alone is normal on
        // plenty of legitimate sections.
        let risk = if has("third-party-authorship-density")
            || (has("commercial-intent-skew") && has("affiliate-density"))
        {
            "high"
        } else if !flags.is_empty() {
            "medium"
        } else {
            "low"
        };

        sections.insert(
            section.clone(),
            json!({
                "page_count": pages.len(),
                "third_party_hits_per_page": (tp * 100.0).round() / 100.0,
                "commerce_hits_per_page": (cm * 100.0).round() / 100.0,
                "affiliate_link_hits_per_page": (af * 100.0).round() / 100.0,
                "flags": flags,
                "risk": risk,
                "sample_urls": pages.iter().take(3).map(|p| p.0.clone()).collect::<Vec<_>>(),
            }),
        );
    }

    // Cross-section drift: a section with more than twice the site mean
    // commercial density is worth flagging even when it clears the absolute
    // threshold on its own.
    if !rates.is_empty() {
        let mean = rates.iter().sum::<f64>() / rates.len() as f64;
        if mean > 0.0 {
            for (_, value) in sections.iter_mut() {
                let rate = value["commerce_hits_per_page"].as_f64().unwrap_or(0.0);
                if rate > 2.0 * mean {
                    let flags = value["flags"].as_array_mut().unwrap();
                    if !flags.iter().any(|f| f == "commercial-intent-drift") {
                        flags.push(json!("commercial-intent-drift"));
                    }
                    if value["risk"] == "low" {
                        value["risk"] = json!("medium");
                    }
                }
            }
        }
    }

    let mut summary: BTreeMap<&str, usize> = BTreeMap::new();
    for value in sections.values() {
        *summary.entry(value["risk"].as_str().unwrap_or("low")).or_insert(0) += 1;
    }
    let overall = if summary.get("high").copied().unwrap_or(0) > 0 {
        "high"
    } else if summary.get("medium").copied().unwrap_or(0) > 0 {
        "medium"
    } else {
        "low"
    };

    let result = json!({
        "pages_audited": rows.len(),
        "errors": errors,
        "by_section": sections,
        "summary": summary,
        "overall_risk": overall,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Overall risk: {overall}");
        println!("Pages audited: {}", rows.len());
        if !errors.is_empty() {
            println!("Errors:        {}", errors.len());
        }
        for (section, value) in result["by_section"].as_object().unwrap() {
            println!(
                "\n  Section {section}  ({} pages)  risk={}",
                value["page_count"],
                value["risk"].as_str().unwrap_or("?")
            );
            println!("    third-party/page: {}", value["third_party_hits_per_page"]);
            println!("    commerce/page:    {}", value["commerce_hits_per_page"]);
            println!("    affiliate/page:   {}", value["affiliate_link_hits_per_page"]);
        }
    }
    code(overall != "high")
}

// ---------------------------------------------------------------- UCP check

const KNOWN_CAPABILITIES: &[(&str, &str)] = &[
    ("dev.ucp.shopping.checkout", "Initiate checkout, return totals + payment intent"),
    ("dev.ucp.shopping.fulfillment", "Quote shipping options + delivery windows"),
    ("dev.ucp.shopping.discount", "Apply promo codes / loyalty discounts"),
    ("dev.ucp.shopping.cart", "Add / remove / update items in agent-managed carts"),
    ("dev.ucp.shopping.catalog", "Search / list products via agent queries"),
    ("dev.ucp.shopping.order", "Order status, lookup, history"),
    ("dev.ucp.shopping.returns", "Return initiation + status"),
];

pub fn ucp(site: &str, probe: bool, timeout: u64, json: bool) -> CmdResult<ExitCode> {
    let normalized = coerce_scheme(site);
    let parsed = Url::parse(&normalized).map_err(|e| Error(e.to_string()))?;
    let discovery = format!(
        "{}://{}/.well-known/ucp",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default()
    );

    let mut report = json!({
        "site": site,
        "discovery_url": discovery,
        "profile_present": false,
        "status_code": serde_json::Value::Null,
        "parse": serde_json::Value::Null,
        "endpoint_probes": [],
        "summary": "",
    });

    if let Err(e) = validate_url_strict(&discovery) {
        report["summary"] = json!(format!("discovery-url-blocked-by-url-safety: {e}"));
        print_json(&report)?;
        return OK;
    }

    let opts = RequestOptions::with_timeout(timeout);
    let resp = match http::get(&discovery, &opts) {
        Ok(r) => r,
        Err(e) => {
            report["summary"] = json!(format!("fetch-failed: {e}"));
            print_json(&report)?;
            return OK;
        }
    };
    report["status_code"] = json!(resp.status);

    if resp.status == 404 {
        report["summary"] = json!("no-ucp-profile (forward-looking opportunity)");
        print_json(&report)?;
        return OK;
    }
    if resp.status >= 400 {
        report["summary"] = json!(format!("http-{} on discovery", resp.status));
        print_json(&report)?;
        return OK;
    }

    report["profile_present"] = json!(true);
    let parsed_profile = parse_ucp_profile(&resp.text());
    report["parse"] = parsed_profile.clone();

    if probe {
        let mut probes = Vec::new();
        if let Some(caps) = parsed_profile["capabilities"].as_array() {
            for cap in caps {
                if let Some(endpoint) = cap["endpoint"].as_str() {
                    probes.push(probe_endpoint(endpoint, timeout));
                }
            }
        }
        report["endpoint_probes"] = json!(probes);
    }

    let n_caps = parsed_profile["capabilities"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let n_issues = parsed_profile["issues"].as_array().map(|a| a.len()).unwrap_or(0);
    report["summary"] = json!(format!(
        "profile-found: {n_caps} capabilities, {n_issues} structural issues"
    ));

    if json {
        print_json(&report)?;
    } else {
        println!("Site: {site}");
        println!("Discovery: {discovery}");
        println!("Status: {}", report["status_code"]);
        println!("Summary: {}", report["summary"].as_str().unwrap_or_default());
        if let Some(caps) = parsed_profile["capabilities"].as_array() {
            println!("Capabilities ({}):", caps.len());
            for c in caps {
                let known = KNOWN_CAPABILITIES
                    .iter()
                    .find(|(id, _)| Some(*id) == c["id"].as_str())
                    .map(|(_, d)| *d)
                    .unwrap_or("(unrecognised capability id)");
                println!(
                    "  - {} (v{}) -> {}  # {known}",
                    c["id"].as_str().unwrap_or("?"),
                    c["version"].as_str().unwrap_or("?"),
                    c["endpoint"].as_str().unwrap_or("?")
                );
            }
        }
    }
    OK
}

fn parse_ucp_profile(payload: &str) -> serde_json::Value {
    let mut issues: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut capabilities: Vec<serde_json::Value> = Vec::new();

    let data: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "valid_json": false,
                "version": null,
                "capabilities": [],
                "merchant": null,
                "issues": [format!("invalid-json: {e}")],
                "unknown_capabilities": [],
            })
        }
    };
    let Some(obj) = data.as_object() else {
        return json!({
            "valid_json": true,
            "version": null,
            "capabilities": [],
            "merchant": null,
            "issues": ["profile-not-object"],
            "unknown_capabilities": [],
        });
    };

    let version = match obj.get("version") {
        None => {
            issues.push("missing-version".into());
            serde_json::Value::Null
        }
        Some(v) if !v.is_string() => {
            issues.push("version-not-string".into());
            serde_json::Value::Null
        }
        Some(v) => v.clone(),
    };

    let merchant = match obj.get("merchant") {
        None => {
            issues.push("missing-merchant".into());
            serde_json::Value::Null
        }
        Some(m) if m.is_object() => {
            if m.get("name").and_then(|n| n.as_str()).unwrap_or("").is_empty() {
                issues.push("merchant-name-empty".into());
            }
            json!({"name": m.get("name"), "id": m.get("id")})
        }
        Some(_) => {
            issues.push("merchant-not-object".into());
            serde_json::Value::Null
        }
    };

    match obj.get("capabilities") {
        None => issues.push("missing-capabilities".into()),
        Some(serde_json::Value::Array(list)) => {
            for (idx, cap) in list.iter().enumerate() {
                let Some(cap_obj) = cap.as_object() else {
                    issues.push(format!("capability-{idx}-not-object"));
                    continue;
                };
                let mut cap_issues: Vec<String> = Vec::new();
                let id = cap_obj.get("id").and_then(|v| v.as_str());
                match id {
                    None | Some("") => cap_issues.push("missing-id".into()),
                    Some(v) if !KNOWN_CAPABILITIES.iter().any(|(k, _)| *k == v) => {
                        unknown.push(v.to_string())
                    }
                    _ => {}
                }
                if cap_obj.get("version").is_none() {
                    cap_issues.push("missing-version".into());
                }
                if cap_obj.get("endpoint").is_none() {
                    cap_issues.push("missing-endpoint".into());
                }
                capabilities.push(json!({
                    "id": cap_obj.get("id"),
                    "version": cap_obj.get("version"),
                    "endpoint": cap_obj.get("endpoint"),
                    "issues": cap_issues,
                }));
            }
        }
        Some(_) => issues.push("capabilities-not-array".into()),
    }

    json!({
        "valid_json": true,
        "version": version,
        "capabilities": capabilities,
        "merchant": merchant,
        "issues": issues,
        "unknown_capabilities": unknown,
    })
}

fn probe_endpoint(url: &str, timeout: u64) -> serde_json::Value {
    if let Err(e) = validate_url_strict(url) {
        return json!({"url": url, "reachable": false, "status_code": null, "error": format!("ssrf-blocked: {e}")});
    }
    match http::get(url, &RequestOptions::with_timeout(timeout)) {
        Ok(r) => json!({
            "url": url,
            "reachable": (200..500).contains(&r.status),
            "status_code": r.status,
            "error": null,
        }),
        Err(e) => json!({"url": url, "reachable": false, "status_code": null, "error": e.to_string()}),
    }
}

// ----------------------------------------------------------------- GBP lint

pub fn gbp_lint(source: &str, is_file: bool, json: bool) -> CmdResult<ExitCode> {
    let body = if is_file {
        std::fs::read_to_string(source)?
    } else {
        let rec = fetch_record(source, 20, true, None);
        match rec.error {
            Some(e) => return err(e),
            None => rec.content.unwrap_or_default(),
        }
    };

    // Generic "Message us" CTAs are common and legitimate, so we only match
    // phrasings that name Google Business explicitly.
    let chat = re(
        r"(?i)\bmessage\s+us\s+(?:on|via|through)\s+google\b|\bchat\s+(?:on|via|with)\s+google\s+(?:business|maps)\b|\bgoogle\s+business\s+chat\b|\bgoogle[-\s]?business[-\s]?messages?\b",
    );
    let business_site = re(r#"(?i)https?://[^/\s"']+\.business\.site(?:/[^\s"']*)?"#);

    let mut chat_hits: Vec<String> = chat.find_iter(&body).map(|m| m.as_str().to_string()).collect();
    chat_hits.sort();
    chat_hits.dedup();
    let mut site_hits: Vec<String> = business_site
        .find_iter(&body)
        .map(|m| m.as_str().to_string())
        .collect();
    site_hits.sort();
    site_hits.dedup();

    let mut findings = Vec::new();
    for hit in &chat_hits {
        findings.push(json!({
            "severity": "Critical",
            "feature": "gbp-chat",
            "match": crate::output::truncate(hit, 200),
            "message": "GBP chat and call-history were fully sunset on 2024-07-31. The CTA does \
                        nothing and breaks user trust. Replace it with a working channel.",
        }));
    }
    for hit in &site_hits {
        findings.push(json!({
            "severity": "Medium",
            "feature": "business-site-url",
            "match": hit,
            "message": "Legacy *.business.site URL found. Verify whether it still resolves and \
                        point it at your actual site if it does not.",
        }));
    }

    let critical = findings.iter().filter(|f| f["severity"] == "Critical").count();
    let medium = findings.iter().filter(|f| f["severity"] == "Medium").count();
    let ok = findings.is_empty();

    let result = json!({
        "ok": ok,
        "findings": findings,
        "summary": {"critical": critical, "high": 0, "medium": medium},
    });

    if json {
        print_json(&result)?;
    } else {
        println!(
            "GBP deprecation lint: {} ({critical} critical, 0 high, {medium} medium)",
            if ok { "PASS" } else { "FAIL" }
        );
        for f in result["findings"].as_array().unwrap() {
            println!(
                "  [{:<8}] {}: {:?}",
                f["severity"].as_str().unwrap_or("?"),
                f["feature"].as_str().unwrap_or("?"),
                f["match"].as_str().unwrap_or("")
            );
            println!("           {}", f["message"].as_str().unwrap_or(""));
        }
    }
    code(ok)
}

// ------------------------------------------------------------ domain history

const DATE_LABELS_CREATED: &[&str] = &[
    "creation date",
    "created on",
    "registered on",
    "registered",
    "domain registration date",
    "registry creation date",
];
const DATE_LABELS_UPDATED: &[&str] = &[
    "updated date",
    "last updated",
    "last modified",
    "domain last updated",
    "registry updated",
];
const DATE_LABELS_EXPIRES: &[&str] = &[
    "expiration date",
    "registry expiry date",
    "expires on",
    "registrar registration expiration date",
];
const REGISTRAR_LABELS: &[&str] = &["registrar", "registrant organization"];

/// Query IANA for the authoritative WHOIS server, then ask that server.
/// Pure TCP/43 — no extra dependency and no reliance on a `whois` binary.
fn whois_query(server: &str, query: &str) -> Option<String> {
    use std::time::Duration;
    let addr = format!("{server}:43");
    let mut stream = TcpStream::connect(&addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.write_all(format!("{query}\r\n").as_bytes()).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn whois(domain: &str) -> Option<(String, String)> {
    let iana = whois_query("whois.iana.org", domain)?;
    let refer = iana
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.to_ascii_lowercase()
                .strip_prefix("refer:")
                .map(|_| l[6..].trim().to_string())
        })
        .filter(|s| !s.is_empty());
    match refer {
        Some(server) => whois_query(&server, domain)
            .map(|text| (text, format!("iana-referral:{server}")))
            .or(Some((iana, "iana".into()))),
        None => Some((iana, "iana".into())),
    }
}

fn extract_field(labels: &[&str], text: &str) -> Option<String> {
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if labels.contains(&key.as_str()) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn parse_whois_date(value: &str) -> Option<String> {
    let v = value.trim();
    // Most registries emit ISO-8601; take the leading date portion.
    if v.len() >= 10 {
        let head = &v[..10];
        if head.as_bytes()[4] == b'-'
            && head.as_bytes()[7] == b'-'
            && head[..4].chars().all(|c| c.is_ascii_digit())
        {
            return Some(head.to_string());
        }
    }
    // dd-Mon-yyyy
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let parts: Vec<&str> = v.split(['-', '.', '/']).collect();
    if parts.len() == 3 {
        if let Some(mi) = months
            .iter()
            .position(|m| parts[1].to_ascii_lowercase().starts_with(m))
        {
            if let (Ok(d), Ok(y)) = (parts[0].parse::<u32>(), parts[2][..4].parse::<i32>()) {
                return Some(format!("{y:04}-{:02}-{d:02}", mi + 1));
            }
        }
        // dd.mm.yyyy
        if let (Ok(d), Ok(m), Ok(y)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2][..4.min(parts[2].len())].parse::<i32>(),
        ) {
            if (1..=31).contains(&d) && (1..=12).contains(&m) {
                return Some(format!("{y:04}-{m:02}-{d:02}"));
            }
        }
    }
    None
}

fn years_since(iso_date: &str) -> Option<f64> {
    let parts: Vec<&str> = iso_date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d) = (
        parts[0].parse::<i32>().ok()?,
        parts[1].parse::<u8>().ok()?,
        parts[2].parse::<u8>().ok()?,
    );
    let created = time::Date::from_calendar_date(y, time::Month::try_from(m).ok()?, d).ok()?;
    let today = time::OffsetDateTime::now_utc().date();
    let days = (today - created).whole_days() as f64;
    Some((days / 365.25 * 100.0).round() / 100.0)
}

pub fn domain_history(
    domain: &str,
    topic: Option<&str>,
    baseline_topic: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let mut notes: Vec<String> = Vec::new();
    let (raw, source) = match whois(domain) {
        Some(v) => (v.0, Some(v.1)),
        None => {
            notes.push("whois unavailable — check egress on TCP/43".into());
            (String::new(), None)
        }
    };

    let created = extract_field(DATE_LABELS_CREATED, &raw).and_then(|v| parse_whois_date(&v));
    let updated = extract_field(DATE_LABELS_UPDATED, &raw).and_then(|v| parse_whois_date(&v));
    let expires = extract_field(DATE_LABELS_EXPIRES, &raw).and_then(|v| parse_whois_date(&v));
    let registrar = extract_field(REGISTRAR_LABELS, &raw);
    let years = created.as_deref().and_then(years_since);

    let topical_shift = match (topic, baseline_topic) {
        (Some(a), Some(b)) => Some(!a.trim().eq_ignore_ascii_case(b.trim())),
        _ => None,
    };

    let risk = match (years, topical_shift) {
        (None, _) => {
            notes.push("no creation date in whois response".into());
            "unknown"
        }
        (Some(y), Some(true)) if y < 2.0 => {
            notes.push("fresh registration with declared topical shift".into());
            "high"
        }
        (Some(y), Some(true)) if y >= 5.0 => {
            notes.push(
                "old registration + topical drift = classic expired-domain abuse pattern".into(),
            );
            "high"
        }
        (Some(_), Some(true)) => {
            notes.push("topical drift detected at moderate registration age".into());
            "medium"
        }
        (Some(y), Some(false)) if y >= 1.0 => "low",
        (Some(_), None) => {
            notes.push("supply --baseline-topic to enable shift detection".into());
            "unknown"
        }
        _ => "unknown",
    };

    let result = json!({
        "domain": domain,
        "whois_source": source,
        "created": created,
        "updated": updated,
        "expires": expires,
        "registrar": registrar,
        "years_registered": years,
        "current_topic": topic,
        "baseline_topic": baseline_topic,
        "topical_shift": topical_shift,
        "risk": risk,
        "notes": notes,
    });

    if json {
        print_json(&result)?;
    } else {
        for (label, key) in [
            ("Domain", "domain"),
            ("WHOIS source", "whois_source"),
            ("Created", "created"),
            ("Updated", "updated"),
            ("Expires", "expires"),
            ("Registrar", "registrar"),
            ("Years registered", "years_registered"),
            ("Topic now", "current_topic"),
            ("Topic baseline", "baseline_topic"),
            ("Topical shift", "topical_shift"),
            ("Risk", "risk"),
        ] {
            println!("{label:<18} {}", result[key]);
        }
        for n in result["notes"].as_array().unwrap() {
            println!("  - {}", n.as_str().unwrap_or_default());
        }
    }
    code(matches!(risk, "low" | "unknown"))
}

// ---------------------------------------------------------------- agent UX

/// How readable a page is to an agent that does not run JavaScript: does the
/// raw HTML carry the content, are there semantic landmarks, is there a
/// machine-readable summary?
pub fn agent_ux(url: &str, json: bool) -> CmdResult<ExitCode> {
    let rec = fetch_record(url, 20, true, None);
    if let Some(e) = rec.error {
        return err(e);
    }
    let body = rec.content.unwrap_or_default();
    let parsed = html::parse(&body, Some(&rec.url));
    let text = html::visible_text(&body);
    let words = html::word_count(&text);
    let (has_ssr, ssr_issues) = html::ssr_assessment(&body, words);

    let mut score = 0i64;
    let mut findings: Vec<serde_json::Value> = Vec::new();
    let mut add = |pass: bool, points: i64, id: &str, message: &str, fix: &str| {
        if pass {
            score += points;
        }
        findings.push(json!({
            "check": id, "pass": pass, "points": if pass { points } else { 0 },
            "max_points": points, "message": message, "fix": fix,
        }));
    };

    add(
        has_ssr && words >= 200,
        25,
        "server-rendered-content",
        "Primary content is present in the raw HTML",
        "Server-render or prerender the main content; agents rarely execute JavaScript.",
    );
    add(
        parsed.title.as_deref().is_some_and(|t| !t.trim().is_empty()),
        10,
        "title",
        "Page has a non-empty <title>",
        "Add a descriptive <title>.",
    );
    add(
        parsed
            .meta_description
            .as_deref()
            .is_some_and(|d| d.trim().len() >= 50),
        10,
        "meta-description",
        "Meta description is present and substantial",
        "Write a 50-160 character meta description summarising the page.",
    );
    add(
        parsed.h1.len() == 1,
        10,
        "single-h1",
        "Exactly one H1 gives an unambiguous topic signal",
        "Use exactly one H1 per page.",
    );
    add(
        !parsed.schema.is_empty(),
        15,
        "structured-data",
        "JSON-LD structured data is present",
        "Add JSON-LD so agents can resolve entities without parsing prose.",
    );
    add(
        body.contains("<main") || body.contains("role=\"main\""),
        10,
        "main-landmark",
        "A <main> landmark isolates primary content",
        "Wrap the primary content in <main>.",
    );
    add(
        parsed.images.iter().all(|i| i.alt.is_some()),
        10,
        "image-alt",
        "Every image carries an alt attribute",
        "Add alt text to every image; agents read alt, not pixels.",
    );
    add(
        !parsed.links.internal.is_empty(),
        10,
        "internal-links",
        "Internal links let an agent traverse the site",
        "Add contextual internal links in the body content.",
    );

    let result = json!({
        "url": rec.url,
        "score": score,
        "word_count": words,
        "has_server_rendered_content": has_ssr,
        "ssr_issues": ssr_issues,
        "findings": findings,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {}", rec.url);
        println!("Agent-readability score: {score}/100");
        for f in result["findings"].as_array().unwrap() {
            let mark = if f["pass"].as_bool().unwrap_or(false) { "PASS" } else { "FAIL" };
            println!(
                "  [{mark}] {:<24} {}",
                f["check"].as_str().unwrap_or(""),
                f["message"].as_str().unwrap_or("")
            );
            if !f["pass"].as_bool().unwrap_or(false) {
                println!("           fix: {}", f["fix"].as_str().unwrap_or(""));
            }
        }
    }
    code(score >= 70)
}

// ---------------------------------------------------------------- hreflang

/// BCP 47 sanity checks. Full registry validation would need a bundled
/// subtag table; these rules catch the mistakes that actually occur:
/// country codes in the language slot, `en-UK` instead of `en-GB`,
/// underscores instead of hyphens, and a missing `x-default`.
fn hreflang_issues(
    entries: &[html::Hreflang],
    page_url: &str,
    canonical: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut issues = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut has_self = false;
    let mut host_variant_self = false;
    let mut has_default = false;

    // A www/apex mismatch is a real problem, but a different one from a
    // missing self-reference — distinguish them rather than reporting the
    // wrong cause.
    let loose = |u: &str| -> String {
        Url::parse(u)
            .map(|p| {
                format!(
                    "{}{}",
                    p.host_str().unwrap_or_default().trim_start_matches("www."),
                    p.path().trim_end_matches('/')
                )
            })
            .unwrap_or_else(|_| u.trim_end_matches('/').to_string())
    };
    let self_keys: Vec<String> = std::iter::once(page_url)
        .chain(canonical)
        .map(loose)
        .collect();

    for e in entries {
        let lang = e.lang.trim();
        *seen.entry(lang.to_ascii_lowercase()).or_insert(0) += 1;

        if lang.eq_ignore_ascii_case("x-default") {
            has_default = true;
        } else {
            if lang.contains('_') {
                issues.push(json!({
                    "severity": "error", "lang": lang,
                    "message": "hreflang uses '_' — BCP 47 requires '-' (e.g. en-GB)",
                }));
            }
            let parts: Vec<&str> = lang.split('-').collect();
            let primary = parts[0];
            if primary.len() != 2 && primary.len() != 3 {
                issues.push(json!({
                    "severity": "error", "lang": lang,
                    "message": "language subtag must be a 2- or 3-letter ISO 639 code",
                }));
            }
            if primary.chars().any(|c| c.is_ascii_uppercase()) {
                issues.push(json!({
                    "severity": "warning", "lang": lang,
                    "message": "language subtag should be lowercase",
                }));
            }
            if parts.len() > 1 {
                let region = parts[parts.len() - 1];
                if region.len() == 2 && region.chars().any(|c| c.is_ascii_lowercase()) {
                    issues.push(json!({
                        "severity": "warning", "lang": lang,
                        "message": "region subtag should be uppercase (e.g. en-GB)",
                    }));
                }
                if region.eq_ignore_ascii_case("UK") {
                    issues.push(json!({
                        "severity": "error", "lang": lang,
                        "message": "'UK' is not an ISO 3166-1 code — use 'GB'",
                    }));
                }
                if region.eq_ignore_ascii_case("EU") || region.eq_ignore_ascii_case("UN") {
                    issues.push(json!({
                        "severity": "error", "lang": lang,
                        "message": format!("'{region}' is not a valid hreflang region"),
                    }));
                }
            }
        }

        match &e.href {
            None => issues.push(json!({
                "severity": "error", "lang": lang, "message": "hreflang entry has no href",
            })),
            Some(href) => {
                if !href.starts_with("http") {
                    issues.push(json!({
                        "severity": "error", "lang": lang, "href": href,
                        "message": "hreflang href must be a fully-qualified absolute URL",
                    }));
                }
                let exact = href.trim_end_matches('/') == page_url.trim_end_matches('/')
                    || canonical.is_some_and(|c| href.trim_end_matches('/') == c.trim_end_matches('/'));
                if exact {
                    has_self = true;
                } else if self_keys.contains(&loose(href)) {
                    host_variant_self = true;
                }
            }
        }
    }

    for (lang, count) in &seen {
        if *count > 1 {
            issues.push(json!({
                "severity": "error", "lang": lang,
                "message": format!("hreflang '{lang}' declared {count} times"),
            }));
        }
    }
    if !entries.is_empty() && !has_self {
        if host_variant_self {
            issues.push(json!({
                "severity": "warning",
                "message": format!(
                    "hreflang points at a different host/path variant of this page \
                     (resolved URL is {page_url}). Align hreflang, canonical, and the \
                     redirect target on one form."
                ),
            }));
        } else {
            issues.push(json!({
                "severity": "error",
                "message": "no self-referencing hreflang — every page in the set must list itself",
            }));
        }
    }
    if !entries.is_empty() && !has_default {
        issues.push(json!({
            "severity": "warning",
            "message": "no x-default entry — add one for unmatched locales",
        }));
    }
    issues
}

pub fn hreflang(url: Option<&str>, file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (body, label) = match (file, url) {
        (Some(p), _) => (std::fs::read_to_string(p)?, url.unwrap_or(p).to_string()),
        (None, Some(u)) => {
            let rec = fetch_record(u, 30, true, None);
            match rec.error {
                Some(e) => return err(e),
                None => (rec.content.unwrap_or_default(), rec.url),
            }
        }
        (None, None) => return err("pass a URL or --file"),
    };

    let parsed = html::parse(&body, Some(&label));
    let issues = hreflang_issues(&parsed.hreflang, &label, parsed.canonical.as_deref());
    let errors = issues.iter().filter(|i| i["severity"] == "error").count();
    let warnings = issues.iter().filter(|i| i["severity"] == "warning").count();

    let result = json!({
        "url": label,
        "entries": parsed.hreflang,
        "entry_count": parsed.hreflang.len(),
        "errors": errors,
        "warnings": warnings,
        "issues": issues,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {label}");
        println!("hreflang entries: {}", parsed.hreflang.len());
        for e in &parsed.hreflang {
            println!("  {:<10} {}", e.lang, e.href.as_deref().unwrap_or("(no href)"));
        }
        println!("Errors: {errors}   Warnings: {warnings}");
        for i in &issues {
            println!(
                "  [{}] {}",
                i["severity"].as_str().unwrap_or("?"),
                i["message"].as_str().unwrap_or("")
            );
        }
    }
    code(errors == 0)
}

// ------------------------------------------------------------ images audit

pub fn images_audit(url: Option<&str>, file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (body, label) = match (file, url) {
        (Some(p), _) => (std::fs::read_to_string(p)?, url.unwrap_or(p).to_string()),
        (None, Some(u)) => {
            let rec = fetch_record(u, 30, true, None);
            match rec.error {
                Some(e) => return err(e),
                None => (rec.content.unwrap_or_default(), rec.url),
            }
        }
        (None, None) => return err("pass a URL or --file"),
    };

    let parsed = html::parse(&body, Some(&label));
    let total = parsed.images.len();
    let missing_alt: Vec<&html::ImageInfo> =
        parsed.images.iter().filter(|i| i.alt.is_none()).collect();
    let empty_alt = parsed
        .images
        .iter()
        .filter(|i| i.alt.as_deref().is_some_and(|a| a.trim().is_empty()))
        .count();
    let missing_dims: Vec<&html::ImageInfo> = parsed
        .images
        .iter()
        .filter(|i| i.width.is_none() || i.height.is_none())
        .collect();
    let lazy: usize = parsed
        .images
        .iter()
        .filter(|i| i.lazy_method != "none")
        .count();
    let modern_formats = parsed
        .images
        .iter()
        .filter(|i| {
            let s = i.src.to_ascii_lowercase();
            s.contains(".webp") || s.contains(".avif")
        })
        .count();

    // The first image on the page is the usual LCP candidate; lazy-loading it
    // delays discovery and directly costs LCP.
    let first_is_lazy = parsed
        .images
        .first()
        .is_some_and(|i| i.lazy_method != "none");

    let mut recs: Vec<String> = Vec::new();
    if !missing_alt.is_empty() {
        recs.push(format!(
            "{} image(s) have no alt attribute. Add descriptive alt text, or alt=\"\" for purely \
             decorative images.",
            missing_alt.len()
        ));
    }
    if !missing_dims.is_empty() {
        recs.push(format!(
            "{} image(s) lack width/height. Set both so the browser reserves space and CLS stays flat.",
            missing_dims.len()
        ));
    }
    if first_is_lazy {
        recs.push(
            "The first image is lazy-loaded. If it is the LCP element, remove lazy loading and \
             add fetchpriority=\"high\"."
                .into(),
        );
    }
    if total > 0 && modern_formats == 0 {
        recs.push("No AVIF/WebP images detected. Modern formats cut image bytes 25-50%.".into());
    }
    if total > 0 && lazy == 0 {
        recs.push("No lazy loading detected. Add loading=\"lazy\" to below-the-fold images.".into());
    }

    let mut score = 100i64;
    if total > 0 {
        score -= (missing_alt.len() as i64 * 100 / total as i64).min(35);
        score -= (missing_dims.len() as i64 * 100 / total as i64).min(25);
        if first_is_lazy {
            score -= 15;
        }
        if modern_formats == 0 {
            score -= 15;
        }
        if lazy == 0 && total > 5 {
            score -= 10;
        }
    }
    score = score.max(0);

    let result = json!({
        "url": label,
        "score": score,
        "total_images": total,
        "missing_alt": missing_alt.len(),
        "empty_alt_decorative": empty_alt,
        "missing_dimensions": missing_dims.len(),
        "lazy_loaded": lazy,
        "modern_format_images": modern_formats,
        "first_image_lazy": first_is_lazy,
        "lazy_methods": parsed.images.iter().fold(BTreeMap::<String, usize>::new(), |mut acc, i| {
            *acc.entry(i.lazy_method.clone()).or_insert(0) += 1;
            acc
        }),
        "images": parsed.images,
        "recommendations": recs,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {label}");
        println!("Score: {score}/100");
        println!("Images: {total}");
        println!("  missing alt:        {}", missing_alt.len());
        println!("  missing dimensions: {}", missing_dims.len());
        println!("  lazy-loaded:        {lazy}");
        println!("  AVIF/WebP:          {modern_formats}");
        for r in result["recommendations"].as_array().unwrap() {
            println!("  - {}", r.as_str().unwrap_or(""));
        }
    }
    code(score >= 70)
}

// -------------------------------------------------------------------- IPTC

/// IPTC DigitalSourceType values that declare AI involvement.
const DIGITAL_SOURCE_TYPES: &[&str] = &[
    "trainedAlgorithmicMedia",
    "compositeWithTrainedAlgorithmicMedia",
    "algorithmicMedia",
    "digitalCapture",
    "composite",
];

pub fn iptc(action: IptcAction) -> CmdResult<ExitCode> {
    match action {
        IptcAction::Audit { path, json } => iptc_audit(&path, json),
        IptcAction::Inject {
            path,
            source_type,
            creator,
            description,
            json,
        } => iptc_inject(&path, &source_type, creator.as_deref(), description.as_deref(), json),
    }
}

/// Sidecar path for an image: `photo.webp` → `photo.webp.xmp`.
fn sidecar_path(image: &std::path::Path) -> std::path::PathBuf {
    let mut s = image.as_os_str().to_os_string();
    s.push(".xmp");
    std::path::PathBuf::from(s)
}

fn iptc_audit(path: &str, json: bool) -> CmdResult<ExitCode> {
    let p = std::path::Path::new(path);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if p.is_dir() {
        for entry in std::fs::read_dir(p)? {
            let entry = entry?;
            let ep = entry.path();
            let ext = ep
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "avif" | "gif" | "tif" | "tiff")
            {
                files.push(ep);
            }
        }
    } else if p.is_file() {
        files.push(p.to_path_buf());
    } else {
        return err(format!("not found: {path}"));
    }
    files.sort();

    let mut rows = Vec::new();
    for f in &files {
        let sidecar = sidecar_path(f);
        let (has_label, source_type) = if sidecar.exists() {
            let text = std::fs::read_to_string(&sidecar).unwrap_or_default();
            let found = DIGITAL_SOURCE_TYPES
                .iter()
                .find(|t| text.contains(**t))
                .map(|t| t.to_string());
            (found.is_some(), found)
        } else {
            (false, None)
        };
        rows.push(json!({
            "path": f.display().to_string(),
            "sidecar": sidecar.display().to_string(),
            "sidecar_present": sidecar.exists(),
            "has_digital_source_type": has_label,
            "digital_source_type": source_type,
        }));
    }

    let labelled = rows
        .iter()
        .filter(|r| r["has_digital_source_type"].as_bool().unwrap_or(false))
        .count();
    let result = json!({
        "path": path,
        "images": files.len(),
        "labelled": labelled,
        "unlabelled": files.len() - labelled,
        "results": rows,
        "note": "seogeo writes and reads XMP sidecars. Embedding into the image container itself \
                 requires exiftool; run `exiftool -tagsfromfile <img>.xmp <img>` to merge.",
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Path: {path}");
        println!("Images: {}  labelled: {labelled}  unlabelled: {}", files.len(), files.len() - labelled);
        for r in result["results"].as_array().unwrap() {
            println!(
                "  [{}] {}",
                if r["has_digital_source_type"].as_bool().unwrap_or(false) { "OK " } else { "GAP" },
                r["path"].as_str().unwrap_or("")
            );
        }
    }
    code(files.is_empty() || labelled == files.len())
}

fn iptc_inject(
    path: &str,
    source_type: &str,
    creator: Option<&str>,
    description: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    if !DIGITAL_SOURCE_TYPES.contains(&source_type) {
        return err(format!(
            "unknown DigitalSourceType {source_type:?}; expected one of {}",
            DIGITAL_SOURCE_TYPES.join(", ")
        ));
    }
    let p = std::path::Path::new(path);
    if !p.is_file() {
        return err(format!("not a file: {path}"));
    }
    let sidecar = sidecar_path(p);

    let creator_block = creator
        .map(|c| {
            format!(
                "   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n",
                xml_escape(c)
            )
        })
        .unwrap_or_default();
    let description_block = description
        .map(|d| {
            format!(
                "   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n",
                xml_escape(d)
            )
        })
        .unwrap_or_default();

    let xmp = format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
          xmlns:Iptc4xmpExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/"
          xmlns:dc="http://purl.org/dc/elements/1.1/">
  <rdf:Description rdf:about="">
   <Iptc4xmpExt:DigitalSourceType>http://cv.iptc.org/newscodes/digitalsourcetype/{source_type}</Iptc4xmpExt:DigitalSourceType>
{creator_block}{description_block}  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#
    );
    std::fs::write(&sidecar, xmp)?;

    let result = json!({
        "path": path,
        "sidecar": sidecar.display().to_string(),
        "digital_source_type": source_type,
        "written": true,
        "next_step": format!("exiftool -tagsfromfile {} {}", sidecar.display(), path),
    });
    if json {
        print_json(&result)?;
    } else {
        println!("Wrote {}", sidecar.display());
        println!("DigitalSourceType: {source_type}");
        println!("To embed into the image itself: exiftool -tagsfromfile {} {}", sidecar.display(), path);
    }
    OK
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hreflang_flags_uk_and_missing_self() {
        let entries = vec![
            html::Hreflang { lang: "en-UK".into(), href: Some("https://a.test/en".into()) },
            html::Hreflang { lang: "fr_FR".into(), href: Some("https://a.test/fr".into()) },
        ];
        let issues = hreflang_issues(&entries, "https://a.test/", None);
        let msgs: Vec<&str> = issues.iter().filter_map(|i| i["message"].as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("'UK' is not an ISO 3166-1 code")));
        assert!(msgs.iter().any(|m| m.contains("BCP 47 requires '-'")));
        assert!(msgs.iter().any(|m| m.contains("no self-referencing hreflang")));
    }

    #[test]
    fn subfolder_extraction() {
        assert_eq!(subfolder("https://a.test/deals/x/y"), "/deals/");
        assert_eq!(subfolder("https://a.test/"), "/");
    }

    #[test]
    fn whois_date_forms() {
        assert_eq!(parse_whois_date("2011-05-04T00:00:00Z").as_deref(), Some("2011-05-04"));
        assert_eq!(parse_whois_date("04-May-2011").as_deref(), Some("2011-05-04"));
        assert_eq!(parse_whois_date("04.05.2011").as_deref(), Some("2011-05-04"));
    }
}

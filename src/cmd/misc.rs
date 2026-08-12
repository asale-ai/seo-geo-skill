//! Assorted commands: IndexNow submission, the ranking-update timeline, the
//! FLOW prompt sync, Unlighthouse, screenshots, PDF deliverables, and the
//! skill/CLI parity map.

use std::process::ExitCode;

use serde_json::{json, Value};
use url::Url;

use crate::chrome;
use crate::http::{self, RequestOptions};
use crate::output::{err, print_json, truncate, CmdResult, Error};
use crate::safety::{coerce_scheme, validate_url_strict};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

/// Bundled so `seo-updates` works offline and cannot drift from the binary.
const GOOGLE_UPDATES: &str = include_str!("../../data/google-updates.json");

/// The command ↔ skill map, checked against the shipped `SKILL.md` files by
/// `tests/skill_cli_parity.rs`. Adding a subcommand without a skill that
/// calls it (or vice versa) fails that test.
pub const COMMAND_MAP: &[(&str, &str)] = &[
    ("url-safety", "seo-technical"),
    ("fetch", "seo, seo-page, geo"),
    ("parse", "seo-page, seo-audit, geo-audit"),
    ("render", "seo-technical, geo-technical"),
    ("sitemap-discovery", "seo-sitemap"),
    ("robots", "seo-technical, geo-crawlers"),
    ("llms-txt", "geo-llmstxt, seo-geo"),
    ("blocks", "geo-citability"),
    ("crawl-sitemap", "seo-sitemap, seo-audit"),
    ("citability", "geo-citability, seo-geo"),
    ("brand-scan", "geo-brand-mentions"),
    ("content-quality", "seo-content, geo-content"),
    ("content-humanize", "seo-content"),
    ("content-verify", "seo-content"),
    ("nlp-analyze", "seo-content, seo-google"),
    ("schema-generate", "seo-schema, geo-schema"),
    ("schema-validate", "seo-schema, geo-schema"),
    ("schema-ecommerce", "seo-ecommerce"),
    ("drift", "seo-drift"),
    ("preload-check", "seo-technical"),
    ("lcp-subparts", "seo-technical, seo-google"),
    ("parasite-risk", "seo-content"),
    ("ucp-check", "seo-ecommerce"),
    ("gbp-lint", "seo-local"),
    ("domain-history", "seo-content"),
    ("agent-ux", "seo-geo, geo-technical"),
    ("hreflang", "seo-hreflang"),
    ("images-audit", "seo-images"),
    ("iptc", "seo-images, seo-image-gen"),
    ("google-auth", "seo-google"),
    ("pagespeed", "seo-technical, seo-google"),
    ("crux-history", "seo-google"),
    ("gsc-query", "seo-google"),
    ("gsc-sitemaps", "seo-google, seo-sitemap"),
    ("gsc-sites", "seo-google"),
    ("gsc-inspect", "seo-google"),
    ("indexing-notify", "seo-google"),
    ("ga4-report", "seo-google"),
    ("keyword-planner", "seo-plan, seo-cluster"),
    ("youtube-search", "seo-geo, geo-brand-mentions"),
    ("google-report", "seo-google, geo-report"),
    ("backlinks-auth", "seo-backlinks"),
    ("moz", "seo-backlinks"),
    ("bing", "seo-backlinks, seo-bing"),
    ("commoncrawl", "seo-backlinks"),
    ("verify-backlinks", "seo-backlinks"),
    ("validate-backlink-report", "seo-backlinks"),
    ("dataforseo-costs", "seo-dataforseo"),
    ("dataforseo-normalize", "seo-dataforseo"),
    ("dataforseo-merchant", "seo-ecommerce, seo-dataforseo"),
    ("indexnow", "seo-technical, seo-bing"),
    ("seo-updates", "seo"),
    ("sync-flow", "seo-flow"),
    ("unlighthouse", "seo-unlighthouse"),
    ("screenshot", "seo-page, seo-sxo"),
    ("crm", "geo-prospect, geo-compare"),
    ("report-pdf", "geo-report-pdf, geo-report"),
    ("install", "seo-geo-skill, geo-update"),
    ("commands", "seo-geo-skill, seo"),
];

// ---------------------------------------------------------------- indexnow

const INDEXNOW_ENDPOINT: &str = "https://api.indexnow.org/indexnow";

pub fn indexnow(
    host: &str,
    urls: &[String],
    urls_file: Option<&str>,
    verify_only: bool,
    key: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let host_url = coerce_scheme(host);
    let parsed = Url::parse(&host_url).map_err(|e| Error(format!("invalid --host: {e}")))?;
    let hostname = parsed
        .host_str()
        .ok_or_else(|| Error("--host has no hostname".into()))?
        .to_string();

    let key = key
        .map(|k| k.to_string())
        .or_else(|| crate::cmd::google::load_config().indexnow_key)
        .ok_or_else(|| {
            Error(
                "No IndexNow key. Pass --key or set INDEXNOW_KEY. The key must also be served at \
                 https://<host>/<key>.txt containing exactly the key."
                    .into(),
            )
        })?;

    // The key file is the ownership proof; a submission without it is
    // rejected, so check it before spending a request.
    let key_location = format!("{}://{hostname}/{key}.txt", parsed.scheme());
    let key_check = match http::get(&key_location, &RequestOptions::with_timeout(20)) {
        Ok(r) if r.status == 200 => {
            let body = r.text();
            let matches = body.trim() == key;
            json!({"url": key_location, "status": r.status, "valid": matches,
                   "error": if matches { Value::Null } else { json!("key file content does not match the key") }})
        }
        Ok(r) => json!({"url": key_location, "status": r.status, "valid": false,
                        "error": format!("key file returned HTTP {}", r.status)}),
        Err(e) => json!({"url": key_location, "status": null, "valid": false,
                         "error": e.to_string()}),
    };

    if verify_only {
        if json {
            print_json(&json!({"host": hostname, "key_file": key_check}))?;
        } else {
            println!("Key file: {key_location}");
            println!("Valid:    {}", key_check["valid"]);
            if let Some(e) = key_check["error"].as_str() {
                println!("Error:    {e}");
            }
        }
        return Ok(if key_check["valid"] == true {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }

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
        return err("pass --urls <url>... or --urls-file <file>");
    }
    if targets.len() > 10_000 {
        return err("IndexNow accepts at most 10,000 URLs per submission");
    }

    // Every URL must be on the declared host or the whole batch is rejected.
    let mut rejected = Vec::new();
    targets.retain(|u| {
        let ok = Url::parse(u)
            .ok()
            .and_then(|p| p.host_str().map(|h| h == hostname))
            .unwrap_or(false);
        if !ok {
            rejected.push(u.clone());
        }
        ok
    });
    if targets.is_empty() {
        return err(format!("no submitted URL is on host {hostname}"));
    }

    let body = json!({
        "host": hostname,
        "key": key,
        "keyLocation": key_location,
        "urlList": targets,
    });
    let resp = http::post_json(INDEXNOW_ENDPOINT, &body, &RequestOptions::with_timeout(30))?;

    let accepted = matches!(resp.status, 200 | 202);
    let result = json!({
        "host": hostname,
        "endpoint": INDEXNOW_ENDPOINT,
        "key_file": key_check,
        "submitted": targets.len(),
        "rejected_wrong_host": rejected,
        "status_code": resp.status,
        "accepted": accepted,
        "response": truncate(&resp.text(), 500),
        "meaning": match resp.status {
            200 => "OK — URLs submitted",
            202 => "Accepted — key validation pending",
            400 => "Bad request — invalid URL format",
            403 => "Forbidden — key not valid for this host",
            422 => "Unprocessable — URLs do not belong to the host, or the key does not match",
            429 => "Too many requests — slow down",
            _ => "Unexpected status",
        },
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Submitted {} URL(s) for {hostname}", targets.len());
        println!("  HTTP {} — {}", resp.status, result["meaning"].as_str().unwrap_or(""));
        if !rejected.is_empty() {
            println!("  {} URL(s) skipped (wrong host)", rejected.len());
        }
    }
    Ok(if accepted { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

// ------------------------------------------------------------- seo updates

pub fn seo_updates(since: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let data: Value = serde_json::from_str(GOOGLE_UPDATES)?;
    let empty = vec![];
    let all = data["updates"].as_array().unwrap_or(&empty);
    let filtered: Vec<&Value> = match since {
        Some(d) => all
            .iter()
            .filter(|u| u["date"].as_str().is_some_and(|x| x >= d))
            .collect(),
        None => all.iter().collect(),
    };

    let result = json!({
        "source_of_truth": data["source_of_truth"],
        "last_verified": data["last_verified"],
        "since": since,
        "count": filtered.len(),
        "updates": filtered,
        "unverified": data["unverified"],
    });

    if json {
        print_json(&result)?;
    } else {
        println!(
            "Google ranking updates ({} entries, verified {})",
            filtered.len(),
            data["last_verified"].as_str().unwrap_or("?")
        );
        for u in &filtered {
            println!(
                "  {}  [{}] {}",
                u["date"].as_str().unwrap_or(""),
                u["kind"].as_str().unwrap_or(""),
                u["name"].as_str().unwrap_or("")
            );
            if let Some(n) = u["notes"].as_str() {
                println!("      {}", truncate(n, 110));
            }
        }
        println!("\nPrimary source: {}", data["source_of_truth"].as_str().unwrap_or(""));
    }
    OK
}

// ---------------------------------------------------------------- sync flow

const FLOW_API_ROOT: &str = "https://api.github.com/repos/AgriciDaniel/flow/contents";

/// Pull the FLOW prompt library (CC BY 4.0) into the seo-flow skill's
/// reference directory. Each file gets an attribution header so the licence
/// travels with the copy.
pub fn sync_flow(dry_run: bool, git_ref: &str, json: bool) -> CmdResult<ExitCode> {
    let dest = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".claude/skills/seo-flow/references/flow-prompts");

    let listing_url = format!("{FLOW_API_ROOT}/prompts?ref={git_ref}");
    validate_url_strict(&listing_url)?;
    let opts = RequestOptions::with_timeout(30)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    let resp = http::get(&listing_url, &opts)?;
    if !(200..300).contains(&resp.status) {
        return err(format!(
            "GitHub returned HTTP {} for the FLOW prompt listing. The upstream repository may be \
             private or renamed.",
            resp.status
        ));
    }
    let listing: Value = serde_json::from_slice(&resp.body)?;
    let empty = vec![];
    let files: Vec<&Value> = listing
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|f| f["type"] == "file" && f["name"].as_str().is_some_and(|n| n.ends_with(".md")))
        .collect();

    let mut written = Vec::new();
    if !dry_run {
        std::fs::create_dir_all(&dest)?;
    }
    for file in &files {
        let name = file["name"].as_str().unwrap_or_default();
        let download = file["download_url"].as_str().unwrap_or_default();
        if download.is_empty() {
            continue;
        }
        if dry_run {
            written.push(json!({"name": name, "action": "would-write"}));
            continue;
        }
        let body = http::get(download, &RequestOptions::with_timeout(30))?;
        let content = format!(
            "<!-- Source: github.com/AgriciDaniel/flow | License: CC BY 4.0 | \
             synced by seogeo on {} -->\n\n{}",
            crate::output::today_utc(),
            body.text()
        );
        let path = dest.join(name);
        std::fs::write(&path, content)?;
        written.push(json!({"name": name, "path": path.display().to_string(), "action": "written"}));
    }

    let result = json!({
        "ref": git_ref,
        "destination": dest.display().to_string(),
        "available": files.len(),
        "written": written.len(),
        "dry_run": dry_run,
        "files": written,
        "license": "CC BY 4.0 — attribution header prepended to every synced file",
    });

    if json {
        print_json(&result)?;
    } else {
        println!(
            "{} {} FLOW prompt(s) -> {}",
            if dry_run { "Would sync" } else { "Synced" },
            files.len(),
            dest.display()
        );
    }
    OK
}

// ------------------------------------------------------------ unlighthouse

pub fn unlighthouse(url: &str, limit: usize, json: bool) -> CmdResult<ExitCode> {
    let (norm, _) = validate_url_strict(&coerce_scheme(url))?;

    let npx = which("npx").ok_or_else(|| {
        Error(
            "Unlighthouse needs Node 18+ with npx on PATH. Install Node, then re-run; the CLI is \
             fetched on first use."
                .into(),
        )
    })?;

    let out_dir = std::env::temp_dir().join(format!(
        "seogeo-unlighthouse-{}",
        norm.replace(['/', ':', '.'], "_")
    ));
    std::fs::create_dir_all(&out_dir)?;

    let status = std::process::Command::new(&npx)
        .args([
            "--yes",
            "unlighthouse-ci",
            "--site",
            &norm,
            "--output-path",
            &out_dir.display().to_string(),
            "--reporter",
            "jsonExpanded",
        ])
        .env("UNLIGHTHOUSE_SCANNER_MAX_ROUTES", limit.to_string())
        .status()
        .map_err(|e| Error(format!("could not run unlighthouse: {e}")))?;

    let report_path = out_dir.join("ci-result.json");
    if !status.success() && !report_path.exists() {
        return err(format!(
            "unlighthouse exited with {status} and wrote no report to {}",
            out_dir.display()
        ));
    }

    let raw = std::fs::read_to_string(&report_path)
        .map_err(|e| Error(format!("could not read {}: {e}", report_path.display())))?;
    let data: Value = serde_json::from_str(&raw)?;

    let result = json!({
        "site": norm,
        "max_routes": limit,
        "report_path": report_path.display().to_string(),
        "report": data,
    });
    if json {
        print_json(&result)?;
    } else {
        println!("Unlighthouse report: {}", report_path.display());
    }
    OK
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

// ------------------------------------------------------------- screenshot

pub fn screenshot(
    url: &str,
    output: &str,
    viewport: &str,
    full_page: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    let (norm, _) = validate_url_strict(&coerce_scheme(url))?;
    let (w, h) = viewport
        .split_once(['x', 'X'])
        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        .ok_or_else(|| Error(format!("invalid --viewport {viewport:?}; expected WIDTHxHEIGHT")))?;

    let path = std::path::Path::new(output);
    chrome::screenshot(&norm, path, w, h, full_page, 15000)?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let result = json!({
        "url": norm,
        "output": output,
        "viewport": {"width": w, "height": h},
        "full_page": full_page,
        "bytes": bytes,
    });
    if json {
        print_json(&result)?;
    } else {
        println!("{output} ({bytes} bytes, {w}x{h})");
    }
    OK
}

// -------------------------------------------------------------- report PDF

/// Convert an audit markdown report into a styled, client-ready document.
/// Markdown → HTML happens in-process; the optional PDF step uses headless
/// Chrome, so there is no pandoc dependency.
pub fn report_pdf(
    input: &str,
    output: Option<&str>,
    brand: Option<&str>,
    score: Option<&str>,
    html_only: bool,
) -> CmdResult<ExitCode> {
    let markdown = std::fs::read_to_string(input)
        .map_err(|e| Error(format!("could not read {input}: {e}")))?;

    let brand = brand
        .map(String::from)
        .or_else(|| first_heading(&markdown))
        .unwrap_or_else(|| "GEO Audit".into());
    let score = score
        .map(String::from)
        .or_else(|| extract_score(&markdown));

    let stem = output
        .map(|o| o.trim_end_matches(".pdf").trim_end_matches(".html").to_string())
        .unwrap_or_else(|| {
            std::path::Path::new(input)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "report".into())
        });
    let html_path = std::path::PathBuf::from(format!("{stem}.html"));
    let pdf_path = std::path::PathBuf::from(format!("{stem}.pdf"));

    let html = render_markdown_report(&markdown, &brand, score.as_deref());
    std::fs::write(&html_path, html)?;

    if html_only {
        println!("{}", html_path.display());
        eprintln!("Wrote {}", html_path.display());
        return OK;
    }

    chrome::print_pdf(&html_path, &pdf_path, 8000)?;
    println!("{}", pdf_path.display());
    eprintln!("Wrote {} and {}", html_path.display(), pdf_path.display());
    OK
}

fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l[2..].trim().to_string())
}

/// Reports write the headline score as `NN/100`; lift the first one.
fn extract_score(md: &str) -> Option<String> {
    let re = regex::Regex::new(r"\b(\d{1,3})\s*/\s*100\b").ok()?;
    re.captures(md).map(|c| c[1].to_string())
}

fn render_markdown_report(markdown: &str, brand: &str, score: Option<&str>) -> String {
    use crate::cmd::drift::esc;
    let body = markdown_to_html(markdown);
    let score_badge = score
        .map(|s| format!("<div class=\"score\"><span>{}</span><small>/100</small></div>", esc(s)))
        .unwrap_or_default();

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{brand_esc}</title>
<style>
:root {{ --navy:#12263f; --navy2:#1e3a5f; --gold:#b8860b; --green:#2d6a4f;
        --amber:#d4740e; --red:#c53030; --blue:#2b6cb0;
        --bg:#fff; --fg:#16191d; --muted:#5b6570; --line:#e3e7eb; --card:#faf9f7; }}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg:#14171a; --fg:#e8eaed; --muted:#9aa4af; --line:#2a2f35; --card:#1c2025; }}
}}
@page {{ size:A4; margin:16mm 14mm; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--fg);
        font:14px/1.7 "Iowan Old Style",Georgia,"Times New Roman",serif; }}
.cover {{ background:linear-gradient(150deg,var(--navy),var(--navy2));
          color:#fff; padding:5rem 3rem; page-break-after:always; }}
.cover h1 {{ font-size:2.4rem; margin:0 0 .6rem; }}
.cover .meta {{ opacity:.82; font-size:.95rem; }}
.score {{ margin-top:2.5rem; display:inline-flex; align-items:baseline; gap:.3rem;
          background:rgba(255,255,255,.12); border:1px solid rgba(255,255,255,.25);
          border-radius:14px; padding:1rem 1.6rem; }}
.score span {{ font-size:3rem; font-weight:700; line-height:1; }}
.score small {{ font-size:1.1rem; opacity:.8; }}
.wrap {{ max-width:920px; margin:0 auto; padding:2.5rem 1.5rem; }}
h1,h2 {{ color:var(--navy2); }}
h2 {{ border-bottom:2px solid var(--line); padding-bottom:.3rem; margin-top:2.4rem;
      page-break-after:avoid; }}
h3 {{ color:var(--gold); margin-top:1.6rem; page-break-after:avoid; }}
.tablewrap {{ overflow-x:auto; }}
table {{ border-collapse:collapse; width:100%; margin:.8rem 0; font-size:.88rem; }}
th,td {{ text-align:left; padding:.45rem .65rem; border-bottom:1px solid var(--line); }}
th {{ background:var(--card); font-weight:600; }}
pre {{ background:var(--card); padding:.8rem 1rem; border-radius:8px; overflow-x:auto;
       font-size:.82rem; }}
code {{ font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.86em; }}
blockquote {{ margin:1rem 0; padding:.6rem 1rem; border-left:4px solid var(--gold);
              background:var(--card); color:var(--muted); }}
ul,ol {{ padding-left:1.3rem; }}
.sev-critical {{ border-left:4px solid var(--red); padding-left:.8rem; }}
.sev-high {{ border-left:4px solid var(--amber); padding-left:.8rem; }}
.sev-medium {{ border-left:4px solid var(--blue); padding-left:.8rem; }}
.sev-low {{ border-left:4px solid var(--green); padding-left:.8rem; }}
</style></head><body>
<div class="cover">
  <h1>{brand_esc}</h1>
  <div class="meta">GEO &amp; SEO audit &middot; {date}</div>
  {score_badge}
</div>
<div class="wrap">{body}</div>
</body></html>"#,
        brand_esc = esc(brand),
        date = crate::output::today_utc(),
    )
}

/// A focused CommonMark subset: headings, lists, tables, code fences, block
/// quotes, links, emphasis, and inline code — everything the audit reports
/// actually emit.
pub fn markdown_to_html(md: &str) -> String {
    use crate::cmd::drift::esc;
    let mut out = String::new();
    let mut in_code = false;
    let mut list: Option<&str> = None;
    let mut in_table = false;
    let mut para: Vec<String> = Vec::new();

    let flush_para = |para: &mut Vec<String>, out: &mut String| {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>", inline_md(&para.join(" "))));
            para.clear();
        }
    };
    let close_list = |list: &mut Option<&str>, out: &mut String| {
        if let Some(tag) = list.take() {
            out.push_str(&format!("</{tag}>"));
        }
    };
    let close_table = |in_table: &mut bool, out: &mut String| {
        if *in_table {
            out.push_str("</tbody></table></div>");
            *in_table = false;
        }
    };

    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            if in_code {
                out.push_str("</code></pre>");
                in_code = false;
            } else {
                flush_para(&mut para, &mut out);
                close_list(&mut list, &mut out);
                close_table(&mut in_table, &mut out);
                out.push_str("<pre><code>");
                in_code = true;
            }
            i += 1;
            continue;
        }
        if in_code {
            out.push_str(&esc(line));
            out.push('\n');
            i += 1;
            continue;
        }

        if trimmed.is_empty() {
            flush_para(&mut para, &mut out);
            close_list(&mut list, &mut out);
            close_table(&mut in_table, &mut out);
            i += 1;
            continue;
        }

        // Tables: a header row followed by a delimiter row.
        if trimmed.starts_with('|')
            && i + 1 < lines.len()
            && lines[i + 1].trim().starts_with('|')
            && lines[i + 1].contains("--")
        {
            flush_para(&mut para, &mut out);
            close_list(&mut list, &mut out);
            out.push_str("<div class=\"tablewrap\"><table><thead><tr>");
            for cell in split_row(trimmed) {
                out.push_str(&format!("<th>{}</th>", inline_md(&cell)));
            }
            out.push_str("</tr></thead><tbody>");
            in_table = true;
            i += 2;
            continue;
        }
        if in_table {
            if trimmed.starts_with('|') {
                out.push_str("<tr>");
                for cell in split_row(trimmed) {
                    out.push_str(&format!("<td>{}</td>", inline_md(&cell)));
                }
                out.push_str("</tr>");
                i += 1;
                continue;
            }
            close_table(&mut in_table, &mut out);
        }

        if let Some(level) = heading_level(trimmed) {
            flush_para(&mut para, &mut out);
            close_list(&mut list, &mut out);
            let text = trimmed[level + 1..].trim();
            let class = severity_class(text);
            out.push_str(&format!(
                "<h{level}{class}>{}</h{level}>",
                inline_md(text)
            ));
            i += 1;
            continue;
        }

        if let Some(quote) = trimmed.strip_prefix("> ") {
            flush_para(&mut para, &mut out);
            out.push_str(&format!("<blockquote>{}</blockquote>", inline_md(quote)));
            i += 1;
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush_para(&mut para, &mut out);
            if list != Some("ul") {
                close_list(&mut list, &mut out);
                out.push_str("<ul>");
                list = Some("ul");
            }
            out.push_str(&format!("<li>{}</li>", inline_md(item)));
            i += 1;
            continue;
        }
        if let Some(rest) = ordered_item(trimmed) {
            flush_para(&mut para, &mut out);
            if list != Some("ol") {
                close_list(&mut list, &mut out);
                out.push_str("<ol>");
                list = Some("ol");
            }
            out.push_str(&format!("<li>{}</li>", inline_md(rest)));
            i += 1;
            continue;
        }

        if trimmed.chars().all(|c| c == '-' || c == '_' || c == '*') && trimmed.len() >= 3 {
            flush_para(&mut para, &mut out);
            close_list(&mut list, &mut out);
            out.push_str("<hr>");
            i += 1;
            continue;
        }

        para.push(trimmed.to_string());
        i += 1;
    }

    flush_para(&mut para, &mut out);
    close_list(&mut list, &mut out);
    close_table(&mut in_table, &mut out);
    if in_code {
        out.push_str("</code></pre>");
    }
    out
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes).then_some(hashes).filter(|_| {
        line.chars().nth(hashes) == Some(' ')
    })
}

fn ordered_item(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

fn severity_class(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    if lower.contains("critical") {
        " class=\"sev-critical\""
    } else if lower.contains("high") {
        " class=\"sev-high\""
    } else if lower.contains("medium") {
        " class=\"sev-medium\""
    } else if lower.contains("low") {
        " class=\"sev-low\""
    } else {
        ""
    }
}

fn split_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Inline markdown: code spans first (their contents must not be re-parsed),
/// then links, bold, and italic.
fn inline_md(text: &str) -> String {
    use crate::cmd::drift::esc;
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find('`') {
        out.push_str(&inline_no_code(&rest[..start]));
        let after = &rest[start + 1..];
        match after.find('`') {
            Some(end) => {
                out.push_str(&format!("<code>{}</code>", esc(&after[..end])));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&inline_no_code(&rest[start..]));
                return out;
            }
        }
    }
    out.push_str(&inline_no_code(rest));
    out
}

fn inline_no_code(text: &str) -> String {
    use crate::cmd::drift::esc;
    let mut s = esc(text);
    let link = regex::Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap();
    s = link.replace_all(&s, "<a href=\"$2\">$1</a>").into_owned();
    let bold = regex::Regex::new(r"\*\*([^*]+)\*\*").unwrap();
    s = bold.replace_all(&s, "<strong>$1</strong>").into_owned();
    let italic = regex::Regex::new(r"(?:^|[^*])\*([^*]+)\*").unwrap();
    s = italic.replace_all(&s, "<em>$1</em>").into_owned();
    s
}

// ---------------------------------------------------------------- commands

pub fn commands(json: bool) -> CmdResult<ExitCode> {
    let rows: Vec<Value> = COMMAND_MAP
        .iter()
        .map(|(cmd, skills)| {
            json!({
                "command": cmd,
                "invocation": format!("seogeo {cmd}"),
                "skills": skills.split(", ").collect::<Vec<_>>(),
            })
        })
        .collect();
    if json {
        print_json(&json!({"count": rows.len(), "commands": rows}))?;
    } else {
        println!("{} commands", rows.len());
        for r in &rows {
            println!(
                "  {:<26} {}",
                r["command"].as_str().unwrap_or(""),
                r["skills"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default()
            );
        }
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_tables_and_lists() {
        let md = "# Title\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n- one\n- two\n\n`code` and **bold**";
        let html = markdown_to_html(md);
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<th>A</th>"));
        assert!(html.contains("<td>1</td>"));
        assert!(html.contains("<li>one</li>"));
        assert!(html.contains("<code>code</code>"));
        assert!(html.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_escapes_html_in_code() {
        let html = markdown_to_html("Use `<script>alert(1)</script>` carefully");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn bundled_updates_parse() {
        let data: Value = serde_json::from_str(GOOGLE_UPDATES).unwrap();
        assert!(data["updates"].as_array().unwrap().len() > 10);
        // Every entry must cite a Google-owned source.
        const GOOGLE_HOSTS: &[&str] = &[
            "google.com", "blog.google", "web.dev", "chrome.com", "developers.google.com",
        ];
        for u in data["updates"].as_array().unwrap() {
            let src = u["source"].as_str().unwrap_or("");
            assert!(
                GOOGLE_HOSTS.iter().any(|h| src.contains(h)),
                "non-Google source: {src}"
            );
        }
    }

    #[test]
    fn score_extracted_from_report() {
        assert_eq!(extract_score("Overall GEO score: 74/100").as_deref(), Some("74"));
        assert_eq!(extract_score("no score here"), None);
    }
}

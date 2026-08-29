//! Self-contained HTML report.
//!
//! The PDF path already exists, but a PDF is where a report goes to die: it
//! cannot be linked, skimmed on a phone, or diffed. This renders one HTML
//! file with the CSS and the charts inlined, so it can be emailed, committed,
//! or dropped on a static host with nothing else beside it.
//!
//! Input is whatever `--json` already produces. Known report shapes get a
//! purpose-built section; anything else still renders, as a readable tree,
//! so a new subcommand does not silently produce a blank page.

use std::process::ExitCode;

use serde_json::Value;

use crate::output::{err, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

pub fn html(inputs: &[String], output: Option<&str>, title: Option<&str>) -> CmdResult<ExitCode> {
    if inputs.is_empty() {
        return err("pass --input <file.json> at least once, or - for stdin");
    }
    let mut sections = String::new();
    let mut heading = title.map(str::to_string);

    for path in inputs {
        let raw = crate::output::read_source(path)?;
        let doc: Value = serde_json::from_str(&raw)
            .map_err(|e| crate::output::Error(format!("{path}: not JSON — {e}")))?;
        if heading.is_none() {
            heading = infer_title(&doc);
        }
        sections.push_str(&render(&doc, path));
    }

    let title = heading.unwrap_or_else(|| "seogeo report".to_string());
    let page = shell(&title, &sections);

    match output {
        Some(path) => {
            std::fs::write(path, &page)?;
            println!("{path}");
            eprintln!("Wrote {path} ({} KB)", page.len() / 1024);
        }
        None => println!("{page}"),
    }
    OK
}

fn infer_title(doc: &Value) -> Option<String> {
    for key in ["brand", "domain", "host", "seed"] {
        if let Some(s) = doc.get(key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    doc.get("summary")
        .and_then(|s| s.get("brand"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Pick the renderer by the fields present rather than by a declared type,
/// because the JSON these commands emit has no type tag and adding one would
/// break every skill that already parses it.
fn render(doc: &Value, source: &str) -> String {
    if doc
        .get("summary")
        .and_then(|s| s.get("mention_rate"))
        .is_some()
    {
        return visibility_section(doc);
    }
    if doc.get("by_purpose").is_some() && doc.get("platforms").is_some() {
        return logs_section(doc);
    }
    if doc.get("summary").and_then(|s| s.get("issues")).is_some() {
        return crawl_section(doc);
    }
    if doc.get("spearman_rho").is_some() {
        return validate_section(doc);
    }
    format!(
        "<section><h2>{}</h2><pre class=\"tree\">{}</pre></section>",
        esc(source),
        esc(&serde_json::to_string_pretty(doc).unwrap_or_default())
    )
}

fn visibility_section(doc: &Value) -> String {
    let s = &doc["summary"];
    let mut out = String::new();
    out.push_str(&format!(
        "<section><h2>Answer-engine visibility</h2><p class=\"sub\">{} · {}</p>",
        esc(s["brand"].as_str().unwrap_or("")),
        esc(doc["timestamp"].as_str().unwrap_or(""))
    ));
    out.push_str("<div class=\"tiles\">");
    out.push_str(&tile(
        "Mention rate",
        &pct(s, "mention_rate"),
        &format!(
            "{} of {} answers",
            s["mentions"].as_u64().unwrap_or(0),
            s["ok"].as_u64().unwrap_or(0)
        ),
    ));
    out.push_str(&tile(
        "Share of voice",
        &pct(s, "share_of_voice"),
        "vs named competitors",
    ));
    out.push_str(&tile(
        "Prominence",
        &format!("{:.2}", s["avg_prominence"].as_f64().unwrap_or(0.0)),
        "1.0 = named first",
    ));
    let retrieval = s["retrieval_probes"].as_u64().unwrap_or(0);
    out.push_str(&tile(
        "Own-domain cites",
        &if retrieval == 0 {
            "—".into()
        } else {
            pct(s, "citation_rate")
        },
        &if retrieval == 0 {
            "no retrieval engine".into()
        } else {
            format!("of {retrieval} sourced answers")
        },
    ));
    out.push_str("</div>");

    if let Some(rows) = s["leaderboard"].as_array() {
        let max = rows
            .iter()
            .filter_map(|r| r["mentions"].as_u64())
            .max()
            .unwrap_or(1)
            .max(1);
        out.push_str("<h3>Who the engines name</h3><div class=\"bars\">");
        for r in rows {
            let n = r["mentions"].as_u64().unwrap_or(0);
            let you = r["is_you"].as_bool().unwrap_or(false);
            out.push_str(&format!(
                "<div class=\"bar-row\"><span class=\"bar-label{}\">{}</span>\
                 <span class=\"bar\"><i style=\"width:{:.1}%\" class=\"{}\"></i></span>\
                 <span class=\"bar-val\">{n}</span></div>",
                if you { " you" } else { "" },
                esc(r["name"].as_str().unwrap_or("")),
                (n as f64 / max as f64) * 100.0,
                if you { "you" } else { "" },
            ));
        }
        out.push_str("</div>");
    }

    if let Some(cites) = s["cited_domains"].as_array().filter(|c| !c.is_empty()) {
        out.push_str("<h3>Most-cited sources</h3><table><tr><th>Domain</th><th>Answers</th></tr>");
        for c in cites.iter().take(15) {
            out.push_str(&format!(
                "<tr><td>{}</td><td class=\"num\">{}</td></tr>",
                esc(c["domain"].as_str().unwrap_or("")),
                c["answers"].as_u64().unwrap_or(0)
            ));
        }
        out.push_str("</table>");
    }

    if let Some(missed) = s["unanswered_prompts"].as_array().filter(|m| !m.is_empty()) {
        out.push_str("<h3>Prompts you did not appear in</h3><ul class=\"plain\">");
        for p in missed.iter().take(25) {
            out.push_str(&format!("<li>{}</li>", esc(p.as_str().unwrap_or(""))));
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");
    out
}

fn logs_section(doc: &Value) -> String {
    let t = &doc["traffic"];
    let p = &doc["by_purpose"];
    let mut out = String::from("<section><h2>AI crawler traffic</h2>");
    out.push_str(&format!(
        "<p class=\"sub\">{} → {} · {} lines read</p>",
        esc(doc["date_range"]["from"].as_str().unwrap_or("?")),
        esc(doc["date_range"]["to"].as_str().unwrap_or("?")),
        doc["lines_read"].as_u64().unwrap_or(0)
    ));
    out.push_str("<div class=\"tiles\">");
    out.push_str(&tile(
        "AI requests",
        &num(t["ai_crawler_requests"].as_u64().unwrap_or(0)),
        &format!(
            "{:.1}% of traffic",
            t["ai_share"].as_f64().unwrap_or(0.0) * 100.0
        ),
    ));
    out.push_str(&tile(
        "Search crawls",
        &num(p["search"].as_u64().unwrap_or(0)),
        "can produce a citation",
    ));
    out.push_str(&tile(
        "Training crawls",
        &num(p["training"].as_u64().unwrap_or(0)),
        "never can",
    ));
    out.push_str(&tile(
        "User-action",
        &num(p["user_action"].as_u64().unwrap_or(0)),
        "a person asked, live",
    ));
    out.push_str("</div>");

    out.push_str("<h3>By platform</h3><table><tr><th>Platform</th><th>Crawls</th><th>Referrals</th><th>Crawl : referral</th><th>Refused</th></tr>");
    for row in doc["platforms"].as_array().unwrap_or(&vec![]) {
        let refused = row["blocked_403"].as_u64().unwrap_or(0)
            + row["rate_limited_429"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num{}\">{}</td></tr>",
            esc(row["platform"].as_str().unwrap_or("")),
            num(row["crawls"].as_u64().unwrap_or(0)),
            num(row["referrals"].as_u64().unwrap_or(0)),
            match row["crawl_to_referral"].as_f64() {
                Some(r) => format!("{r:.0} : 1"),
                None => "—".into(),
            },
            if refused > 0 { " bad" } else { "" },
            refused
        ));
    }
    out.push_str("</table>");
    out.push_str(&findings_list(&doc["findings"]));

    if let Some(cov) = doc
        .get("coverage")
        .filter(|c| c.get("sitemap_urls").is_some())
    {
        out.push_str(&format!(
            "<h3>Sitemap coverage</h3><p>{} of {} sitemap URLs have been fetched by an AI crawler ({:.1}%).</p>",
            cov["fetched_by_ai_crawlers"].as_u64().unwrap_or(0),
            cov["sitemap_urls"].as_u64().unwrap_or(0),
            cov["coverage"].as_f64().unwrap_or(0.0) * 100.0
        ));
        if let Some(miss) = cov["never_fetched_sample"]
            .as_array()
            .filter(|m| !m.is_empty())
        {
            out.push_str("<ul class=\"plain\">");
            for m in miss.iter().take(20) {
                out.push_str(&format!("<li>{}</li>", esc(m.as_str().unwrap_or(""))));
            }
            out.push_str("</ul>");
        }
    }
    out.push_str("</section>");
    out
}

fn crawl_section(doc: &Value) -> String {
    let s = &doc["summary"];
    let mut out = format!(
        "<section><h2>Site crawl</h2><p class=\"sub\">{} · {} pages</p><div class=\"tiles\">",
        esc(doc["host"].as_str().unwrap_or("")),
        doc["crawled"].as_u64().unwrap_or(0)
    );
    out.push_str(&tile(
        "Pages OK",
        &num(s["pages_ok"].as_u64().unwrap_or(0)),
        "reachable and parsed",
    ));
    out.push_str(&tile(
        "Failed",
        &num(s["pages_failed"].as_u64().unwrap_or(0)),
        "4xx, 5xx, or unreachable",
    ));
    out.push_str(&tile(
        "Avg words",
        &num(s["avg_word_count"].as_u64().unwrap_or(0)),
        "per page",
    ));
    out.push_str(&tile(
        "With schema",
        &num(s["pages_with_schema"].as_u64().unwrap_or(0)),
        "structured data present",
    ));
    out.push_str("</div><h3>Issues</h3><table><tr><th>Severity</th><th>Issue</th><th>Count</th><th>Why it matters</th></tr>");
    for i in s["issues"].as_array().unwrap_or(&vec![]) {
        out.push_str(&format!(
            "<tr><td><span class=\"pill {sev}\">{sev}</span></td><td>{}</td><td class=\"num\">{}</td><td class=\"why\">{}</td></tr>",
            esc(i["issue"].as_str().unwrap_or("")),
            i["count"].as_u64().unwrap_or(0),
            esc(i["why"].as_str().unwrap_or("")),
            sev = esc(i["severity"].as_str().unwrap_or("info")),
        ));
    }
    out.push_str("</table></section>");
    out
}

fn validate_section(doc: &Value) -> String {
    let v = &doc["verdict"];
    let mut out = format!(
        "<section><h2>Does the citability score predict citations?</h2><p class=\"sub\">{}</p><div class=\"tiles\">",
        esc(doc["domain"].as_str().unwrap_or(""))
    );
    out.push_str(&tile(
        "Spearman rho",
        &match doc["spearman_rho"].as_f64() {
            Some(r) => format!("{r:+.2}"),
            None => "n/a".into(),
        },
        "score vs times cited",
    ));
    out.push_str(&tile(
        "Pages cited",
        &num(doc["pages_cited"].as_u64().unwrap_or(0)),
        &format!("of {} scored", doc["pages_scored"].as_u64().unwrap_or(0)),
    ));
    out.push_str(&tile(
        "Cited mean",
        &format!(
            "{:.1}",
            doc["mean_citability_cited"].as_f64().unwrap_or(0.0)
        ),
        "citability of cited pages",
    ));
    out.push_str(&tile(
        "Uncited mean",
        &format!(
            "{:.1}",
            doc["mean_citability_uncited"].as_f64().unwrap_or(0.0)
        ),
        "citability of the rest",
    ));
    out.push_str(&format!(
        "</div><p class=\"verdict\">{}</p><p class=\"sub\">{}</p>",
        esc(v["summary"].as_str().unwrap_or("")),
        esc(v["caveat"].as_str().unwrap_or(""))
    ));
    out.push_str("</section>");
    out
}

fn findings_list(findings: &Value) -> String {
    let Some(items) = findings.as_array().filter(|f| !f.is_empty()) else {
        return String::new();
    };
    let mut out = String::from("<h3>Findings</h3><ul class=\"findings\">");
    for f in items {
        out.push_str(&format!(
            "<li><span class=\"pill {sev}\">{sev}</span> <strong>{}</strong> — {}<br><span class=\"why\">{}</span></li>",
            esc(f["finding"].as_str().unwrap_or("")),
            esc(f["detail"].as_str().unwrap_or("")),
            esc(f["why"].as_str().unwrap_or("")),
            sev = esc(f["severity"].as_str().unwrap_or("info")),
        ));
    }
    out.push_str("</ul>");
    out
}

fn tile(label: &str, value: &str, note: &str) -> String {
    format!(
        "<div class=\"tile\"><span class=\"tile-label\">{}</span>\
         <span class=\"tile-value\">{}</span><span class=\"tile-note\">{}</span></div>",
        esc(label),
        esc(value),
        esc(note)
    )
}

fn pct(v: &Value, key: &str) -> String {
    format!("{:.1}%", v[key].as_f64().unwrap_or(0.0) * 100.0)
}

fn num(n: u64) -> String {
    // Thousands separators, because six-digit crawl counts are unreadable raw.
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One file, no external requests. The palette follows the reader's system
/// theme rather than forcing a light page into a dark terminal setup.
fn shell(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
:root {{
  --bg:#fbfbfa; --fg:#1a1a19; --muted:#6b6b68; --line:#e4e4e1; --card:#fff;
  --accent:#2f6f4e; --bad:#a33; --warn:#a76b12; --good:#2f6f4e;
}}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg:#161614; --fg:#eceae6; --muted:#9a978f; --line:#2c2b28; --card:#1e1d1a;
           --accent:#7fc79c; --bad:#e37b7b; --warn:#dfae5e; --good:#7fc79c; }}
}}
* {{ box-sizing:border-box }}
body {{ margin:0; background:var(--bg); color:var(--fg); font:15px/1.6 ui-sans-serif,-apple-system,"Segoe UI",Roboto,sans-serif; }}
main {{ max-width:960px; margin:0 auto; padding:48px 24px 96px }}
h1 {{ font-size:28px; margin:0 0 4px; letter-spacing:-.02em }}
h2 {{ font-size:20px; margin:0 0 2px; letter-spacing:-.01em }}
h3 {{ font-size:14px; text-transform:uppercase; letter-spacing:.08em; color:var(--muted); margin:32px 0 10px }}
section {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:24px; margin:24px 0 }}
.sub {{ color:var(--muted); margin:0 0 16px; font-size:14px }}
.tiles {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:12px; margin:16px 0 }}
.tile {{ border:1px solid var(--line); border-radius:10px; padding:14px; display:flex; flex-direction:column; gap:2px }}
.tile-label {{ font-size:12px; color:var(--muted); text-transform:uppercase; letter-spacing:.06em }}
.tile-value {{ font-size:26px; font-weight:600; letter-spacing:-.02em }}
.tile-note {{ font-size:12px; color:var(--muted) }}
table {{ width:100%; border-collapse:collapse; font-size:14px; display:block; overflow-x:auto }}
th {{ text-align:left; font-size:12px; text-transform:uppercase; letter-spacing:.06em; color:var(--muted); font-weight:500; padding:8px 10px; border-bottom:1px solid var(--line) }}
td {{ padding:8px 10px; border-bottom:1px solid var(--line); vertical-align:top }}
td.num {{ text-align:right; font-variant-numeric:tabular-nums; white-space:nowrap }}
td.num.bad {{ color:var(--bad); font-weight:600 }}
td.why, .why {{ color:var(--muted); font-size:13px }}
.pill {{ display:inline-block; padding:1px 8px; border-radius:999px; font-size:11px; text-transform:uppercase; letter-spacing:.05em; border:1px solid currentColor }}
.pill.critical {{ color:var(--bad) }} .pill.warning {{ color:var(--warn) }}
.pill.info {{ color:var(--muted) }} .pill.good {{ color:var(--good) }}
.bars {{ display:flex; flex-direction:column; gap:6px }}
.bar-row {{ display:grid; grid-template-columns:180px 1fr 48px; align-items:center; gap:10px; font-size:14px }}
.bar-label.you {{ font-weight:700 }}
.bar {{ background:var(--line); border-radius:4px; height:14px; overflow:hidden }}
.bar i {{ display:block; height:100%; background:var(--muted); border-radius:4px }}
.bar i.you {{ background:var(--accent) }}
.bar-val {{ text-align:right; font-variant-numeric:tabular-nums; color:var(--muted) }}
ul.plain, ul.findings {{ margin:0; padding-left:18px }}
ul.plain li {{ color:var(--muted); font-size:13px; margin:2px 0 }}
ul.findings li {{ margin:10px 0; list-style:none; margin-left:-18px }}
.verdict {{ font-size:16px; margin:16px 0 4px }}
pre.tree {{ background:var(--bg); border:1px solid var(--line); border-radius:8px; padding:14px; overflow-x:auto; font-size:12px }}
footer {{ color:var(--muted); font-size:12px; margin-top:32px; text-align:center }}
</style></head><body><main>
<h1>{title}</h1>
<p class="sub">Generated by seogeo {version} · {when}</p>
{body}
<footer>Every number here came from a request this machine made. Nothing was uploaded.</footer>
</main></body></html>"#,
        title = esc(title),
        body = body,
        version = env!("CARGO_PKG_VERSION"),
        when = crate::output::now_utc(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escapes_markup_from_untrusted_page_content() {
        let out = esc("<script>alert(1)</script>&\"");
        assert!(!out.contains('<') && !out.contains('>'));
        assert!(out.contains("&amp;") && out.contains("&quot;"));
    }

    #[test]
    fn thousands_separator() {
        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(1234), "1,234");
        assert_eq!(num(1234567), "1,234,567");
    }

    #[test]
    fn unknown_shape_still_renders() {
        let out = render(&json!({"anything": 1}), "x.json");
        assert!(out.contains("anything"));
    }

    #[test]
    fn visibility_shape_is_detected() {
        let doc = json!({"summary": {"mention_rate": 0.5, "leaderboard": [], "cited_domains": []}});
        assert!(render(&doc, "v.json").contains("Answer-engine visibility"));
    }

    #[test]
    fn page_is_self_contained() {
        let page = shell("t", "<section>x</section>");
        assert!(!page.contains("http://") && !page.contains("https://"));
        assert!(page.contains("<style>"));
    }
}

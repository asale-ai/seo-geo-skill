//! SEO drift monitoring: baseline capture, 17-rule comparison, history, and
//! a standalone HTML report.
//!
//! Baselines live in SQLite under the user cache directory so a comparison
//! run needs no server and survives across sessions.

use std::process::ExitCode;

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::cli::DriftAction;
use crate::cmd::core::fetch_record;
use crate::html;
use crate::output::{err, now_utc, print_json, truncate, CmdResult, Error};
use crate::safety::{coerce_scheme, validate_url};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

const UTM_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
];

pub fn db_path() -> std::path::PathBuf {
    crate::paths::cache_dir().join("drift").join("baselines.db")
}

/// Canonical form for baseline matching: lowercase scheme/host, no default
/// port, sorted query with UTM stripped, no trailing slash.
pub fn normalize_url(input: &str) -> String {
    let raw = coerce_scheme(input);
    let Ok(mut u) = Url::parse(&raw) else {
        return raw;
    };
    let scheme = u.scheme().to_ascii_lowercase();
    if (scheme == "http" && u.port() == Some(80)) || (scheme == "https" && u.port() == Some(443)) {
        let _ = u.set_port(None);
    }
    let mut pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !UTM_PARAMS.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.sort();
    {
        let mut qp = u.query_pairs_mut();
        qp.clear();
        for (k, v) in &pairs {
            qp.append_pair(k, v);
        }
    }
    if pairs.is_empty() {
        u.set_query(None);
    }
    u.set_fragment(None);

    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = u.path().trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{scheme}://{host}{port}{path}{query}")
}

pub fn url_hash(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(normalize_url(url).as_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

fn hash_content(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

fn init_db() -> CmdResult<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS baselines (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            url_hash TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            title TEXT,
            meta_description TEXT,
            canonical TEXT,
            robots TEXT,
            h1 TEXT,
            h2_json TEXT,
            h3_json TEXT,
            schema_json TEXT,
            og_json TEXT,
            cwv_json TEXT,
            html_hash TEXT,
            schema_hash TEXT,
            status_code INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_url_hash ON baselines(url_hash);
        CREATE TABLE IF NOT EXISTS comparisons (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            url_hash TEXT NOT NULL,
            baseline_id INTEGER NOT NULL,
            timestamp TEXT NOT NULL,
            results_json TEXT NOT NULL,
            critical_count INTEGER DEFAULT 0,
            warning_count INTEGER DEFAULT 0,
            info_count INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_comp_url_hash ON comparisons(url_hash);
        "#,
    )?;
    Ok(conn)
}

pub fn run(action: DriftAction) -> CmdResult<ExitCode> {
    match action {
        DriftAction::Baseline {
            url,
            skip_cwv,
            json,
        } => baseline(&url, skip_cwv, json),
        DriftAction::Compare {
            url,
            skip_cwv,
            baseline_id,
            json,
        } => compare(&url, skip_cwv, baseline_id, json),
        DriftAction::History { url, limit, json } => history(&url, limit, json),
        DriftAction::Report { input, output } => report(&input, &output),
    }
}

struct PageState {
    parsed: html::ParsedPage,
    html_hash: String,
    status_code: Option<u16>,
}

fn capture_state(url: &str) -> CmdResult<PageState> {
    let rec = fetch_record(url, 60, true, None);
    if let Some(e) = rec.error {
        return err(format!("Fetch failed: {e}"));
    }
    let body = rec.content.unwrap_or_default();
    Ok(PageState {
        parsed: html::parse(&body, Some(&rec.url)),
        html_hash: hash_content(&body),
        status_code: rec.status_code,
    })
}

/// Field CWV for the baseline. Requires a Google API key; when none is
/// configured the baseline is still captured, just without CWV.
fn capture_cwv(url: &str) -> Option<Value> {
    let key = crate::cmd::google::api_key()?;
    crate::cmd::google::crux_record(url, "PHONE", &key)
        .ok()
        .map(|record| {
            json!({
                "performance_score": Value::Null,
                "field_metrics": record,
            })
        })
}

// ---------------------------------------------------------------- baseline

pub fn baseline(url: &str, skip_cwv: bool, json: bool) -> CmdResult<ExitCode> {
    if !validate_url(&coerce_scheme(url)) {
        return err("URL rejected: only public http/https URLs are accepted (SSRF protection)");
    }
    let state = capture_state(url)?;
    let cwv = if skip_cwv { None } else { capture_cwv(url) };

    let p = &state.parsed;
    let schema_json = serde_json::to_string(&p.schema)?;
    let schema_hash = if p.schema.is_empty() {
        None
    } else {
        Some(hash_content(&schema_json))
    };
    let now = now_utc();
    let norm = normalize_url(url);
    let uhash = url_hash(url);
    let h1 = p.h1.first().cloned();

    let conn = init_db()?;
    conn.execute(
        "INSERT INTO baselines (url, url_hash, timestamp, title, meta_description, canonical,
            robots, h1, h2_json, h3_json, schema_json, og_json, cwv_json, html_hash,
            schema_hash, status_code)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            norm,
            uhash,
            now,
            p.title,
            p.meta_description,
            p.canonical,
            p.meta_robots,
            h1,
            serde_json::to_string(&p.h2)?,
            serde_json::to_string(&p.h3)?,
            schema_json,
            serde_json::to_string(&p.open_graph)?,
            cwv.as_ref().map(|c| c.to_string()),
            state.html_hash,
            schema_hash,
            state.status_code,
        ],
    )?;
    let baseline_id = conn.last_insert_rowid();

    let output = json!({
        "status": "ok",
        "baseline_id": baseline_id,
        "url": norm,
        "timestamp": now,
        "db": db_path().display().to_string(),
        "summary": {
            "title": p.title,
            "meta_description": p.meta_description.as_deref().map(|d| truncate(d, 80)),
            "canonical": p.canonical,
            "robots": p.meta_robots,
            "h1": h1,
            "h2_count": p.h2.len(),
            "h3_count": p.h3.len(),
            "schema_count": p.schema.len(),
            "og_tag_count": p.open_graph.len(),
            "cwv_captured": cwv.is_some(),
            "status_code": state.status_code,
            "html_hash": format!("{}...", &state.html_hash[..12]),
        },
    });

    if json {
        print_json(&output)?;
    } else {
        println!("Baseline #{baseline_id} captured for {norm}");
        println!("  title:  {}", p.title.as_deref().unwrap_or("(none)"));
        println!("  h1:     {}", h1.as_deref().unwrap_or("(none)"));
        println!("  schema: {} block(s)", p.schema.len());
        println!(
            "  cwv:    {}",
            if cwv.is_some() { "captured" } else { "skipped" }
        );
    }
    OK
}

// ------------------------------------------------------ similarity (ratio)

/// Ratcliff/Obershelp similarity, matching Python's
/// `difflib.SequenceMatcher.ratio()` — the metric the original rule set was
/// tuned against.
pub fn similarity_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches = matching_chars(&a, &b);
    2.0 * matches as f64 / total as f64
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (a_start, b_start, size) = longest_match(a, b);
    if size == 0 {
        return 0;
    }
    size + matching_chars(&a[..a_start], &b[..b_start])
        + matching_chars(&a[a_start + size..], &b[b_start + size..])
}

fn longest_match(a: &[char], b: &[char]) -> (usize, usize, usize) {
    let mut best = (0usize, 0usize, 0usize);
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        for (j, bc) in b.iter().enumerate() {
            curr[j + 1] = if ac == bc { prev[j] + 1 } else { 0 };
            if curr[j + 1] > best.2 {
                best = (i + 1 - curr[j + 1], j + 1 - curr[j + 1], curr[j + 1]);
            }
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.iter_mut().for_each(|v| *v = 0);
    }
    best
}

// ----------------------------------------------------------------- compare

/// Types Google no longer surfaces as search rich results. Removing them is
/// a WARNING, not a CRITICAL, because there is no rich result to lose.
const RETIRED_SCHEMA_TYPES: &[&str] = &["FAQPage", "HowTo", "Dataset"];

fn schema_types(blocks: &[Value]) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    for b in blocks {
        let values = match &b["@type"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect(),
            _ => vec![],
        };
        for v in values {
            let short = v.rsplit(['/', '#']).next().unwrap_or(&v).to_string();
            if !short.is_empty() && !types.contains(&short) {
                types.push(short);
            }
        }
    }
    types.sort();
    types
}

fn finding(
    rule: &str,
    severity: &str,
    triggered: bool,
    old: Value,
    new: Value,
    message: String,
) -> Value {
    json!({
        "rule": rule,
        "severity": severity,
        "triggered": triggered,
        "old_value": old,
        "new_value": new,
        "message": message,
    })
}

struct Baseline {
    id: i64,
    timestamp: String,
    title: Option<String>,
    meta_description: Option<String>,
    canonical: Option<String>,
    robots: Option<String>,
    h1: Option<String>,
    h2: Vec<String>,
    schema: Vec<Value>,
    og: serde_json::Map<String, Value>,
    cwv: Option<Value>,
    html_hash: Option<String>,
    schema_hash: Option<String>,
    status_code: Option<i64>,
}

fn load_baseline(conn: &Connection, uhash: &str, id: Option<i64>) -> CmdResult<Option<Baseline>> {
    let sql = if id.is_some() {
        "SELECT id,timestamp,title,meta_description,canonical,robots,h1,h2_json,schema_json,\
         og_json,cwv_json,html_hash,schema_hash,status_code FROM baselines \
         WHERE id = ?1 AND url_hash = ?2"
    } else {
        "SELECT id,timestamp,title,meta_description,canonical,robots,h1,h2_json,schema_json,\
         og_json,cwv_json,html_hash,schema_hash,status_code FROM baselines \
         WHERE url_hash = ?1 ORDER BY id DESC LIMIT 1"
    };
    let mut stmt = conn.prepare(sql)?;
    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<Baseline> {
        let h2: String = row
            .get::<_, Option<String>>(7)?
            .unwrap_or_else(|| "[]".into());
        let schema: String = row
            .get::<_, Option<String>>(8)?
            .unwrap_or_else(|| "[]".into());
        let og: String = row
            .get::<_, Option<String>>(9)?
            .unwrap_or_else(|| "{}".into());
        let cwv: Option<String> = row.get(10)?;
        Ok(Baseline {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            title: row.get(2)?,
            meta_description: row.get(3)?,
            canonical: row.get(4)?,
            robots: row.get(5)?,
            h1: row.get(6)?,
            h2: serde_json::from_str(&h2).unwrap_or_default(),
            schema: serde_json::from_str(&schema).unwrap_or_default(),
            og: serde_json::from_str(&og).unwrap_or_default(),
            cwv: cwv.and_then(|c| serde_json::from_str(&c).ok()),
            html_hash: row.get(11)?,
            schema_hash: row.get(12)?,
            status_code: row.get(13)?,
        })
    };
    let result = match id {
        Some(bid) => stmt.query_row(params![bid, uhash], map_row).ok(),
        None => stmt.query_row(params![uhash], map_row).ok(),
    };
    Ok(result)
}

pub fn compare(
    url: &str,
    skip_cwv: bool,
    baseline_id: Option<i64>,
    json: bool,
) -> CmdResult<ExitCode> {
    if !validate_url(&coerce_scheme(url)) {
        return err("URL rejected: only public http/https URLs are accepted (SSRF protection)");
    }
    let uhash = url_hash(url);
    let norm = normalize_url(url);
    let conn = init_db()?;

    let Some(base) = load_baseline(&conn, &uhash, baseline_id)? else {
        let extra = baseline_id
            .map(|b| format!(" (baseline_id={b})"))
            .unwrap_or_default();
        return err(format!(
            "No baseline found for {norm}{extra}. Run `seogeo drift baseline {norm}` first."
        ));
    };

    let state = capture_state(url)?;
    let cur = &state.parsed;
    let current_cwv = if skip_cwv { None } else { capture_cwv(url) };

    let mut findings: Vec<Value> = Vec::new();

    // --- CRITICAL (rules 1-8) ---
    let old_types = schema_types(&base.schema);
    let retired_only = !old_types.is_empty()
        && old_types
            .iter()
            .all(|t| RETIRED_SCHEMA_TYPES.contains(&t.as_str()));
    let schema_removed = !base.schema.is_empty() && cur.schema.is_empty();
    findings.push(finding(
        "schema_removed",
        if schema_removed && retired_only {
            "WARNING"
        } else {
            "CRITICAL"
        },
        schema_removed,
        json!(format!("{} schema block(s)", base.schema.len())),
        json!("0 schema blocks"),
        if schema_removed {
            schema_removal_message(&old_types)
        } else {
            "Schema presence unchanged.".into()
        },
    ));

    let canonical_changed =
        base.canonical.is_some() && cur.canonical.is_some() && base.canonical != cur.canonical;
    findings.push(finding(
        "canonical_changed",
        "CRITICAL",
        canonical_changed,
        json!(base.canonical),
        json!(cur.canonical),
        if canonical_changed {
            format!(
                "Canonical URL changed from {:?} to {:?}. Verify this is intentional.",
                base.canonical.as_deref().unwrap_or(""),
                cur.canonical.as_deref().unwrap_or("")
            )
        } else {
            "Canonical URL unchanged.".into()
        },
    ));

    let canonical_removed =
        base.canonical.is_some() && cur.canonical.as_deref().unwrap_or("").is_empty();
    findings.push(finding(
        "canonical_removed",
        "CRITICAL",
        canonical_removed,
        json!(base.canonical),
        Value::Null,
        if canonical_removed {
            "Canonical tag has been removed. Google will guess the canonical, often incorrectly."
                .into()
        } else {
            "Canonical tag presence unchanged.".into()
        },
    ));

    let old_robots = base.robots.clone().unwrap_or_default().to_lowercase();
    let new_robots = cur.meta_robots.clone().unwrap_or_default().to_lowercase();
    let noindex_added = !old_robots.contains("noindex") && new_robots.contains("noindex");
    findings.push(finding(
        "noindex_added",
        "CRITICAL",
        noindex_added,
        json!(base.robots),
        json!(cur.meta_robots),
        if noindex_added {
            "A 'noindex' directive has been added. The page will drop out of search within days."
                .into()
        } else {
            "Robots directives unchanged regarding noindex.".into()
        },
    ));

    let h1_removed = base.h1.as_deref().is_some_and(|h| !h.is_empty()) && cur.h1.is_empty();
    findings.push(finding(
        "h1_removed",
        "CRITICAL",
        h1_removed,
        json!(base.h1),
        Value::Null,
        if h1_removed {
            "H1 heading has been removed. The primary topic signal is gone.".into()
        } else {
            "H1 presence unchanged.".into()
        },
    ));

    let old_h1 = base.h1.clone().unwrap_or_default();
    let new_h1 = cur.h1.first().cloned().unwrap_or_default();
    let (h1_changed, h1_msg) = if old_h1.is_empty() || new_h1.is_empty() {
        (false, "H1 comparison skipped (one side empty).".to_string())
    } else {
        let ratio = similarity_ratio(&old_h1, &new_h1);
        if ratio < 0.5 {
            (
                true,
                format!("H1 changed significantly (similarity: {:.0}%). Verify keyword targeting is preserved.", ratio * 100.0),
            )
        } else {
            (
                false,
                format!(
                    "H1 text is similar enough (similarity: {:.0}%).",
                    ratio * 100.0
                ),
            )
        }
    };
    findings.push(finding(
        "h1_changed",
        "CRITICAL",
        h1_changed,
        json!(old_h1),
        json!(new_h1),
        h1_msg,
    ));

    let title_removed = base.title.as_deref().is_some_and(|t| !t.is_empty())
        && cur.title.as_deref().unwrap_or("").is_empty();
    findings.push(finding(
        "title_removed",
        "CRITICAL",
        title_removed,
        json!(base.title),
        Value::Null,
        if title_removed {
            "Title tag has been removed. Google will auto-generate one, often poorly.".into()
        } else {
            "Title tag presence unchanged.".into()
        },
    ));

    let old_status = base.status_code;
    let new_status = state.status_code.map(|s| s as i64);
    let status_error =
        old_status.is_some_and(|s| (200..400).contains(&s)) && new_status.is_some_and(|s| s >= 400);
    findings.push(finding(
        "status_code_error",
        "CRITICAL",
        status_error,
        json!(old_status),
        json!(new_status),
        if status_error {
            format!(
                "Page now returns HTTP {} (was {}). Rankings will drop within days.",
                new_status.unwrap_or(0),
                old_status.unwrap_or(0)
            )
        } else {
            format!("Status code: {:?} -> {:?}.", old_status, new_status)
        },
    ));

    // --- WARNING (rules 9-14) ---
    let old_title = base.title.clone().unwrap_or_default().trim().to_string();
    let new_title = cur.title.clone().unwrap_or_default().trim().to_string();
    let title_changed = !old_title.is_empty() && !new_title.is_empty() && old_title != new_title;
    findings.push(finding(
        "title_changed",
        "WARNING",
        title_changed,
        json!(old_title),
        json!(new_title),
        if title_changed {
            "Title tag text has changed. Monitor CTR in Search Console over two weeks.".into()
        } else {
            "Title text unchanged.".into()
        },
    ));

    let old_desc = base
        .meta_description
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let new_desc = cur
        .meta_description
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let desc_changed = !old_desc.is_empty() && !new_desc.is_empty() && old_desc != new_desc;
    findings.push(finding(
        "meta_description_changed",
        "WARNING",
        desc_changed,
        json!(truncate(&old_desc, 120)),
        json!(truncate(&new_desc, 120)),
        if desc_changed {
            "Meta description has changed. Verify it includes target keywords and a CTA.".into()
        } else {
            "Meta description unchanged.".into()
        },
    ));

    findings.push(cwv_regression_finding(
        base.cwv.as_ref(),
        current_cwv.as_ref(),
    ));
    findings.push(perf_score_finding(base.cwv.as_ref(), current_cwv.as_ref()));

    let og_removed = !base.og.is_empty() && cur.open_graph.is_empty();
    findings.push(finding(
        "og_tags_removed",
        "WARNING",
        og_removed,
        json!(base.og.keys().collect::<Vec<_>>()),
        json!([] as [&str; 0]),
        if og_removed {
            "All Open Graph tags have been removed. Social sharing will show generic previews."
                .into()
        } else {
            "OG tags presence unchanged.".into()
        },
    ));

    let new_schema_str = serde_json::to_string(&cur.schema)?;
    let new_schema_hash = if cur.schema.is_empty() {
        None
    } else {
        Some(hash_content(&new_schema_str))
    };
    let schema_modified = base.schema_hash.is_some()
        && new_schema_hash.is_some()
        && base.schema_hash != new_schema_hash;
    findings.push(finding(
        "schema_modified",
        "WARNING",
        schema_modified,
        json!(base
            .schema_hash
            .as_deref()
            .map(|h| format!("{}...", &h[..12]))),
        json!(new_schema_hash
            .as_deref()
            .map(|h| format!("{}...", &h[..12]))),
        if schema_modified {
            "Schema/JSON-LD content has been modified. Re-validate with `seogeo schema-validate`."
                .into()
        } else {
            "Schema content hash unchanged.".into()
        },
    ));

    // --- INFO (rules 15-17) ---
    let schema_added = base.schema.is_empty() && !cur.schema.is_empty();
    findings.push(finding(
        "schema_added",
        "INFO",
        schema_added,
        json!("0 schema blocks"),
        json!(format!("{} schema block(s)", cur.schema.len())),
        if schema_added {
            "New structured data added. Validate with `seogeo schema-validate`.".into()
        } else {
            "No new schema added.".into()
        },
    ));

    let h2_changed = base.h2 != cur.h2;
    findings.push(finding(
        "h2_structure_changed",
        "INFO",
        h2_changed,
        json!(format!("{} H2s", base.h2.len())),
        json!(format!("{} H2s", cur.h2.len())),
        if h2_changed {
            format!(
                "H2 heading structure changed ({} -> {} headings).",
                base.h2.len(),
                cur.h2.len()
            )
        } else {
            "H2 structure unchanged.".into()
        },
    ));

    let content_changed = base
        .html_hash
        .as_deref()
        .is_some_and(|h| h != state.html_hash);
    findings.push(finding(
        "content_hash_changed",
        "INFO",
        content_changed,
        json!(base
            .html_hash
            .as_deref()
            .map(|h| format!("{}...", &h[..12]))),
        json!(format!("{}...", &state.html_hash[..12])),
        if content_changed {
            "Page content has changed (HTML body hash differs from baseline).".into()
        } else {
            "Page content hash unchanged.".into()
        },
    ));

    let triggered: Vec<&Value> = findings.iter().filter(|f| f["triggered"] == true).collect();
    let untriggered: Vec<&Value> = findings.iter().filter(|f| f["triggered"] != true).collect();
    let count = |sev: &str| triggered.iter().filter(|f| f["severity"] == sev).count();
    let (critical, warning, info) = (count("CRITICAL"), count("WARNING"), count("INFO"));
    let now = now_utc();

    let result = json!({
        "status": "ok",
        "url": norm,
        "baseline_id": base.id,
        "baseline_timestamp": base.timestamp,
        "comparison_timestamp": now,
        "summary": {
            "total_rules": findings.len(),
            "triggered": triggered.len(),
            "critical": critical,
            "warning": warning,
            "info": info,
        },
        "triggered_findings": triggered,
        "untriggered_findings": untriggered,
        "current_status_code": state.status_code,
        "cwv_compared": current_cwv.is_some(),
    });

    conn.execute(
        "INSERT INTO comparisons (url, url_hash, baseline_id, timestamp, results_json,
            critical_count, warning_count, info_count) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            norm,
            uhash,
            base.id,
            now,
            result.to_string(),
            critical as i64,
            warning as i64,
            info as i64
        ],
    )?;

    if json {
        print_json(&result)?;
    } else {
        println!("Drift vs baseline #{} ({})", base.id, base.timestamp);
        println!("  {critical} critical, {warning} warning, {info} info");
        for f in &triggered {
            println!(
                "  [{}] {}: {}",
                f["severity"].as_str().unwrap_or(""),
                f["rule"].as_str().unwrap_or(""),
                f["message"].as_str().unwrap_or("")
            );
        }
        if triggered.is_empty() {
            println!("  No drift detected. Page matches baseline.");
        }
    }
    Ok(if critical == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn schema_removal_message(types: &[String]) -> String {
    if types.is_empty() {
        return "All JSON-LD structured data has been removed. Validate schema types individually \
                before assuming rich-result impact."
            .into();
    }
    let retired: Vec<&String> = types
        .iter()
        .filter(|t| RETIRED_SCHEMA_TYPES.contains(&t.as_str()))
        .collect();
    let active: Vec<&String> = types
        .iter()
        .filter(|t| !RETIRED_SCHEMA_TYPES.contains(&t.as_str()))
        .collect();
    let names = |v: &[&String]| v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    if !active.is_empty() && !retired.is_empty() {
        format!(
            "Structured data removed for {}. Check rich-result eligibility for the non-retired \
             types; FAQPage, HowTo, and Dataset are not a rich-result loss.",
            types.join(", ")
        )
    } else if !active.is_empty() {
        format!(
            "Structured data removed for {}. Check current rich-result eligibility before \
             assuming impact.",
            names(&active)
        )
    } else {
        format!(
            "Removed JSON-LD types ({}) are retired for Google Search rich results. Keep them \
             only if they are useful elsewhere.",
            names(&retired)
        )
    }
}

const FIELD_CWV_METRICS: &[(&str, &str)] = &[
    ("largest_contentful_paint", "LCP"),
    ("interaction_to_next_paint", "INP"),
    ("cumulative_layout_shift", "CLS"),
];

fn field_p75(field: &Value, metric: &str) -> Option<f64> {
    for key in [
        format!("url_{metric}"),
        format!("origin_{metric}"),
        metric.to_string(),
    ] {
        let data = &field[&key];
        if let Some(v) = data["p75"].as_f64().or_else(|| data["value"].as_f64()) {
            return Some(v);
        }
        if let Some(s) = data["p75"].as_str().or_else(|| data["value"].as_str()) {
            if let Ok(v) = s.parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

/// `{metric: p75}` for the three field metrics the drift rules compare.
fn cwv_snapshot(field: &Value) -> Value {
    let mut map = serde_json::Map::new();
    for (metric, _) in FIELD_CWV_METRICS {
        map.insert(metric.to_string(), json!(field_p75(field, metric)));
    }
    Value::Object(map)
}

fn cwv_regression_finding(old: Option<&Value>, new: Option<&Value>) -> Value {
    let (Some(old), Some(new)) = (old, new) else {
        return finding(
            "cwv_regressed",
            "WARNING",
            false,
            Value::Null,
            Value::Null,
            "CWV comparison skipped (data unavailable).".into(),
        );
    };
    let (old_field, new_field) = (&old["field_metrics"], &new["field_metrics"]);
    if old_field.is_null() || new_field.is_null() {
        return finding(
            "cwv_regressed",
            "WARNING",
            false,
            Value::Null,
            Value::Null,
            "CWV field comparison skipped (field data unavailable).".into(),
        );
    }

    let mut regressions = Vec::new();
    for (metric, label) in FIELD_CWV_METRICS {
        if let (Some(o), Some(n)) = (field_p75(old_field, metric), field_p75(new_field, metric)) {
            if o > 0.0 {
                let pct = (n - o) / o;
                if pct > 0.20 {
                    let formatted = if *metric == "cumulative_layout_shift" {
                        format!("{label}: {o:.3} -> {n:.3} (+{:.0}%)", pct * 100.0)
                    } else {
                        format!("{label}: {o:.0} -> {n:.0} (+{:.0}%)", pct * 100.0)
                    };
                    regressions.push(formatted);
                }
            }
        }
    }
    let triggered = !regressions.is_empty();
    finding(
        "cwv_regressed",
        "WARNING",
        triggered,
        cwv_snapshot(old_field),
        cwv_snapshot(new_field),
        if triggered {
            format!("CWV regressions detected: {}", regressions.join("; "))
        } else {
            "No significant CWV regressions.".into()
        },
    )
}

fn perf_score_finding(old: Option<&Value>, new: Option<&Value>) -> Value {
    let (Some(old), Some(new)) = (old, new) else {
        return finding(
            "perf_score_dropped",
            "WARNING",
            false,
            Value::Null,
            Value::Null,
            "Performance score comparison skipped.".into(),
        );
    };
    let (o, n) = (
        old["performance_score"].as_f64(),
        new["performance_score"].as_f64(),
    );
    let (Some(o), Some(n)) = (o, n) else {
        return finding(
            "perf_score_dropped",
            "WARNING",
            false,
            json!(o),
            json!(n),
            "Performance score unavailable.".into(),
        );
    };
    let drop = o - n;
    let triggered = drop >= 10.0;
    finding(
        "perf_score_dropped",
        "WARNING",
        triggered,
        json!(o),
        json!(n),
        if triggered {
            format!("Performance score dropped {drop:.0} points ({o:.0} -> {n:.0}).")
        } else {
            format!("Performance score: {o:.0} -> {n:.0}.")
        },
    )
}

// ----------------------------------------------------------------- history

pub fn history(url: &str, limit: usize, json: bool) -> CmdResult<ExitCode> {
    let uhash = url_hash(url);
    let norm = normalize_url(url);
    let conn = init_db()?;

    let mut stmt = conn.prepare(
        "SELECT id, timestamp, title, status_code FROM baselines WHERE url_hash = ?1 \
         ORDER BY id DESC LIMIT ?2",
    )?;
    let baselines: Vec<Value> = stmt
        .query_map(params![uhash, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "timestamp": row.get::<_, String>(1)?,
                "title": row.get::<_, Option<String>>(2)?,
                "status_code": row.get::<_, Option<i64>>(3)?,
            }))
        })?
        .filter_map(Result::ok)
        .collect();

    let mut stmt = conn.prepare(
        "SELECT id, baseline_id, timestamp, critical_count, warning_count, info_count \
         FROM comparisons WHERE url_hash = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let comparisons: Vec<Value> = stmt
        .query_map(params![uhash, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "baseline_id": row.get::<_, i64>(1)?,
                "timestamp": row.get::<_, String>(2)?,
                "critical": row.get::<_, i64>(3)?,
                "warning": row.get::<_, i64>(4)?,
                "info": row.get::<_, i64>(5)?,
            }))
        })?
        .filter_map(Result::ok)
        .collect();

    let result = json!({
        "url": norm,
        "db": db_path().display().to_string(),
        "baselines": baselines,
        "comparisons": comparisons,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {norm}");
        println!("\nBaselines ({}):", baselines.len());
        for b in &baselines {
            println!(
                "  #{:<4} {}  status={}  {}",
                b["id"],
                b["timestamp"].as_str().unwrap_or(""),
                b["status_code"],
                truncate(b["title"].as_str().unwrap_or("(no title)"), 60)
            );
        }
        println!("\nComparisons ({}):", comparisons.len());
        for c in &comparisons {
            println!(
                "  #{:<4} {}  vs #{}  critical={} warning={} info={}",
                c["id"],
                c["timestamp"].as_str().unwrap_or(""),
                c["baseline_id"],
                c["critical"],
                c["warning"],
                c["info"]
            );
        }
        if baselines.is_empty() {
            println!("\nNo baseline yet — run `seogeo drift baseline {norm}`.");
        }
    }
    OK
}

// ------------------------------------------------------------------ report

pub fn report(input: &str, output: &str) -> CmdResult<ExitCode> {
    let raw = std::fs::read_to_string(input)
        .map_err(|e| Error(format!("could not read {input}: {e}")))?;
    let data: Value = serde_json::from_str(&raw)?;

    let url = data["url"].as_str().unwrap_or("(unknown)");
    let summary = &data["summary"];
    let empty = vec![];
    let triggered = data["triggered_findings"].as_array().unwrap_or(&empty);
    let untriggered = data["untriggered_findings"].as_array().unwrap_or(&empty);

    let mut rows = String::new();
    for f in triggered {
        let sev = f["severity"].as_str().unwrap_or("INFO");
        rows.push_str(&format!(
            r#"<tr class="sev-{}"><td><span class="badge">{}</span></td><td><code>{}</code></td>
               <td>{}</td><td class="val">{}</td><td class="val">{}</td></tr>"#,
            sev.to_lowercase(),
            sev,
            esc(f["rule"].as_str().unwrap_or("")),
            esc(f["message"].as_str().unwrap_or("")),
            esc(&value_text(&f["old_value"])),
            esc(&value_text(&f["new_value"])),
        ));
    }
    if triggered.is_empty() {
        rows.push_str(
            r#"<tr><td colspan="5" class="ok">No drift detected — the page matches its baseline.</td></tr>"#,
        );
    }

    let mut passed = String::new();
    for f in untriggered {
        passed.push_str(&format!(
            "<li><code>{}</code> — {}</li>",
            esc(f["rule"].as_str().unwrap_or("")),
            esc(f["message"].as_str().unwrap_or(""))
        ));
    }

    let html_out = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SEO drift report — {url_esc}</title>
<style>
:root {{ --bg:#ffffff; --fg:#16191d; --muted:#5b6570; --line:#e3e7eb;
        --crit:#c53030; --warn:#d4740e; --info:#2b6cb0; --ok:#2d6a4f; --card:#faf9f7; }}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg:#14171a; --fg:#e8eaed; --muted:#9aa4af; --line:#2a2f35; --card:#1c2025; }}
}}
* {{ box-sizing:border-box; }}
body {{ margin:0; padding:2rem 1.25rem; background:var(--bg); color:var(--fg);
        font:15px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif; }}
.wrap {{ max-width:1000px; margin:0 auto; }}
h1 {{ font-size:1.5rem; margin:0 0 .25rem; }}
.sub {{ color:var(--muted); margin:0 0 1.5rem; font-size:.9rem; }}
.cards {{ display:flex; flex-wrap:wrap; gap:.75rem; margin-bottom:1.5rem; }}
.card {{ flex:1 1 140px; background:var(--card); border:1px solid var(--line);
         border-radius:10px; padding:.9rem 1rem; }}
.card .n {{ font-size:1.6rem; font-weight:600; }}
.card .l {{ color:var(--muted); font-size:.8rem; text-transform:uppercase; letter-spacing:.04em; }}
.tablewrap {{ overflow-x:auto; border:1px solid var(--line); border-radius:10px; }}
table {{ border-collapse:collapse; width:100%; min-width:720px; }}
th,td {{ text-align:left; padding:.6rem .8rem; border-bottom:1px solid var(--line); vertical-align:top; }}
th {{ background:var(--card); font-size:.78rem; text-transform:uppercase; letter-spacing:.04em; color:var(--muted); }}
tr:last-child td {{ border-bottom:0; }}
.badge {{ font-size:.7rem; font-weight:700; padding:.15rem .45rem; border-radius:5px; color:#fff; }}
.sev-critical .badge {{ background:var(--crit); }}
.sev-warning .badge {{ background:var(--warn); }}
.sev-info .badge {{ background:var(--info); }}
.val {{ font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.8rem;
        color:var(--muted); max-width:230px; overflow-wrap:anywhere; }}
.ok {{ color:var(--ok); }}
details {{ margin-top:1.5rem; }}
summary {{ cursor:pointer; color:var(--muted); }}
ul {{ padding-left:1.2rem; color:var(--muted); font-size:.88rem; }}
code {{ font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.85em; }}
</style></head><body><div class="wrap">
<h1>SEO drift report</h1>
<p class="sub">{url_esc} &middot; baseline #{baseline_id} captured {baseline_ts} &middot; compared {compared_ts}</p>
<div class="cards">
  <div class="card"><div class="n">{critical}</div><div class="l">Critical</div></div>
  <div class="card"><div class="n">{warning}</div><div class="l">Warning</div></div>
  <div class="card"><div class="n">{info}</div><div class="l">Info</div></div>
  <div class="card"><div class="n">{total}</div><div class="l">Rules run</div></div>
</div>
<div class="tablewrap"><table>
<thead><tr><th>Severity</th><th>Rule</th><th>What changed</th><th>Baseline</th><th>Now</th></tr></thead>
<tbody>{rows}</tbody></table></div>
<details><summary>Rules that did not trigger ({untriggered_n})</summary><ul>{passed}</ul></details>
</div></body></html>"#,
        url_esc = esc(url),
        baseline_id = data["baseline_id"],
        baseline_ts = esc(data["baseline_timestamp"].as_str().unwrap_or("")),
        compared_ts = esc(data["comparison_timestamp"].as_str().unwrap_or("")),
        critical = summary["critical"],
        warning = summary["warning"],
        info = summary["info"],
        total = summary["total_rules"],
        untriggered_n = untriggered.len(),
    );

    std::fs::write(output, html_out)?;
    println!("{output}");
    eprintln!("Wrote drift report to {output}");
    OK
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => truncate(s, 120),
        other => truncate(&other.to_string(), 120),
    }
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_utm_and_trailing_slash() {
        assert_eq!(
            normalize_url("HTTPS://Example.COM:443/path/?utm_source=x&b=2&a=1#frag"),
            "https://example.com/path?a=1&b=2"
        );
        assert_eq!(normalize_url("example.com"), "https://example.com/");
    }

    #[test]
    fn similarity_matches_difflib() {
        // difflib.SequenceMatcher(None, "abcd", "abcd").ratio() == 1.0
        assert!((similarity_ratio("abcd", "abcd") - 1.0).abs() < 1e-9);
        // ("abcd", "bcde") -> 2*3/8 = 0.75
        assert!((similarity_ratio("abcd", "bcde") - 0.75).abs() < 1e-9);
        // ("Buy widgets", "Widget pricing guide") -> low
        assert!(similarity_ratio("Buy widgets now", "Totally different heading") < 0.5);
    }

    #[test]
    fn schema_types_flatten_arrays() {
        let blocks = vec![
            json!({"@type": "Product"}),
            json!({"@type": ["Article", "https://schema.org/NewsArticle"]}),
        ];
        assert_eq!(
            schema_types(&blocks),
            vec!["Article", "NewsArticle", "Product"]
        );
    }
}

//! Answer-engine visibility: probe, measure, store, trend.
//!
//! This is the half of GEO the rest of the binary could only give advice
//! about. `citability` says a passage *should* be quotable; `visibility run`
//! asks ChatGPT, Perplexity, Gemini, Claude, or a local Ollama model the
//! questions a buyer would actually type, and records whether the brand came
//! back.
//!
//! Every run lands in SQLite under the data directory, because visibility is
//! a trend: one snapshot says nothing, and the question a client asks is
//! "better or worse than last month?".

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::cli::VisibilityAction;
use crate::llm::{self, Probe, Provider};
use crate::output::{err, money, now_utc, print_json, truncate, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

pub fn db_path() -> std::path::PathBuf {
    crate::paths::data_dir().join("visibility.db")
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
        CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            brand TEXT NOT NULL,
            brand_key TEXT NOT NULL,
            domain TEXT,
            label TEXT,
            providers_json TEXT NOT NULL,
            competitors_json TEXT NOT NULL,
            prompt_count INTEGER NOT NULL,
            probe_count INTEGER NOT NULL,
            ok_count INTEGER NOT NULL,
            mention_rate REAL NOT NULL,
            share_of_voice REAL NOT NULL,
            avg_prominence REAL NOT NULL,
            citation_rate REAL NOT NULL,
            cost_usd REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_runs_brand ON runs(brand_key, timestamp);
        CREATE TABLE IF NOT EXISTS probes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            live_search INTEGER NOT NULL,
            prompt TEXT NOT NULL,
            ok INTEGER NOT NULL,
            mentioned INTEGER NOT NULL,
            prominence REAL NOT NULL,
            cited_own INTEGER NOT NULL,
            competitors_json TEXT NOT NULL,
            citations_json TEXT NOT NULL,
            answer TEXT NOT NULL,
            cost_usd REAL NOT NULL,
            error TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_probes_run ON probes(run_id);
        "#,
    )?;
    Ok(conn)
}

pub fn brand_key(brand: &str) -> String {
    brand.trim().to_ascii_lowercase()
}

// --------------------------------------------------------------- mentions

/// Where a name appears in an answer, and how early.
#[derive(Debug, Clone, Copy)]
pub struct Mention {
    pub found: bool,
    /// 1.0 when the name opens the answer, approaching 0.0 at the end.
    /// Engines list the best-known option first, so position is signal.
    pub prominence: f64,
}

/// Case-insensitive whole-word search.
///
/// Substring matching would count "Notion" inside "notionally" and, worse,
/// count a two-letter brand inside half the answer. Boundaries are checked on
/// `char` so accented and CJK names behave.
pub fn find_mention(answer: &str, name: &str) -> Mention {
    let needle = name.trim().to_lowercase();
    if needle.is_empty() || answer.is_empty() {
        return Mention {
            found: false,
            prominence: 0.0,
        };
    }
    let hay: Vec<char> = answer.to_lowercase().chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    if pat.len() > hay.len() {
        return Mention {
            found: false,
            prominence: 0.0,
        };
    }
    let boundary = |c: Option<&char>| match c {
        None => true,
        Some(c) => !(c.is_alphanumeric() || *c == '_'),
    };
    for start in 0..=(hay.len() - pat.len()) {
        if hay[start..start + pat.len()] != pat[..] {
            continue;
        }
        let before = start.checked_sub(1).map(|i| &hay[i]);
        let after = hay.get(start + pat.len());
        if boundary(before) && boundary(after) {
            let prominence = 1.0 - (start as f64 / hay.len().max(1) as f64);
            return Mention {
                found: true,
                prominence: (prominence * 1000.0).round() / 1000.0,
            };
        }
    }
    Mention {
        found: false,
        prominence: 0.0,
    }
}

/// Per-probe verdict: was the brand named, was its site cited, who else showed up.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Verdict {
    pub provider: String,
    pub model: String,
    pub live_search: bool,
    pub prompt: String,
    pub ok: bool,
    pub mentioned: bool,
    pub prominence: f64,
    /// True when a live-search engine cited the brand's own domain. Always
    /// false on model-knowledge providers, which return no sources.
    pub cited_own_domain: bool,
    pub competitors_mentioned: Vec<String>,
    pub citations: Vec<String>,
    pub cost_usd: f64,
    pub latency_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub answer: String,
}

pub fn judge(
    p: &Probe,
    brand: &str,
    aliases: &[String],
    domain: Option<&str>,
    competitors: &[String],
) -> Verdict {
    let mut best = find_mention(&p.answer, brand);
    for alias in aliases {
        let m = find_mention(&p.answer, alias);
        if m.found && (!best.found || m.prominence > best.prominence) {
            best = m;
        }
    }
    let cited_own = domain
        .and_then(llm::cite_domain_of_host)
        .map(|d| {
            p.citations
                .iter()
                .filter_map(|c| llm::cite_domain(c))
                .any(|c| c == d || c.ends_with(&format!(".{d}")))
        })
        .unwrap_or(false);

    let competitors_mentioned = competitors
        .iter()
        .filter(|c| find_mention(&p.answer, c).found)
        .cloned()
        .collect();

    Verdict {
        provider: p.provider.clone(),
        model: p.model.clone(),
        live_search: p.live_search,
        prompt: p.prompt.clone(),
        ok: p.ok,
        mentioned: best.found,
        prominence: best.prominence,
        cited_own_domain: cited_own,
        competitors_mentioned,
        citations: p.citations.clone(),
        cost_usd: p.cost_usd,
        latency_ms: p.latency_ms,
        error: p.error.clone(),
        answer: p.answer.clone(),
    }
}

// --------------------------------------------------------------- seogeo ask

pub fn ask(
    prompt: &str,
    provider_spec: Option<&str>,
    model: Option<&str>,
    brand: Option<&str>,
    timeout: u64,
    json: bool,
) -> CmdResult<ExitCode> {
    let providers = llm::resolve(provider_spec).map_err(crate::output::Error)?;
    let probes: Vec<Probe> = providers
        .iter()
        .map(|p| llm::probe(p, model, prompt, timeout))
        .collect();

    let rows: Vec<Value> = probes
        .iter()
        .map(|p| {
            let mention = brand.map(|b| find_mention(&p.answer, b));
            json!({
                "provider": p.provider,
                "model": p.model,
                "live_search": p.live_search,
                "ok": p.ok,
                "answer": p.answer,
                "citations": p.citations,
                "citation_domains": domains_of(&p.citations),
                "mentioned": mention.map(|m| m.found),
                "prominence": mention.map(|m| m.prominence),
                "input_tokens": p.input_tokens,
                "output_tokens": p.output_tokens,
                "cost_usd": p.cost_usd,
                "latency_ms": p.latency_ms,
                "error": p.error,
            })
        })
        .collect();

    let cost = money(probes.iter().map(|p| p.cost_usd).sum());
    if json {
        print_json(&json!({
            "prompt": prompt,
            "timestamp": now_utc(),
            "brand": brand,
            "results": rows,
            "cost_usd": cost,
        }))?;
    } else {
        for p in &probes {
            println!("── {} ({})", p.provider, p.model);
            match &p.error {
                Some(e) => println!("   error: {e}"),
                None => {
                    println!("{}", p.answer.trim());
                    if !p.citations.is_empty() {
                        println!("   sources: {}", p.citations.join(", "));
                    }
                    if let Some(b) = brand {
                        let m = find_mention(&p.answer, b);
                        println!(
                            "   {b}: {}",
                            if m.found {
                                format!("mentioned (prominence {:.2})", m.prominence)
                            } else {
                                "not mentioned".to_string()
                            }
                        );
                    }
                }
            }
        }
        eprintln!("cost ${cost:.5}");
    }
    if probes.iter().all(|p| !p.ok) {
        return Ok(ExitCode::from(1));
    }
    OK
}

fn domains_of(citations: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = citations
        .iter()
        .filter_map(|c| llm::cite_domain(c))
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

// --------------------------------------------------------- seogeo visibility

pub fn run_action(action: VisibilityAction) -> CmdResult<ExitCode> {
    match action {
        VisibilityAction::Prompts {
            domain,
            brand,
            topics,
            count,
            json,
        } => prompts(&domain, brand.as_deref(), &topics, count, json),
        VisibilityAction::Run {
            brand,
            domain,
            prompts: prompts_file,
            prompt,
            provider,
            model,
            competitors,
            aliases,
            label,
            concurrency,
            timeout,
            dry_run,
            json,
        } => run(
            &brand,
            domain.as_deref(),
            prompts_file.as_deref(),
            &prompt,
            provider.as_deref(),
            model.as_deref(),
            &competitors,
            &aliases,
            label.as_deref(),
            concurrency,
            timeout,
            dry_run,
            json,
        ),
        VisibilityAction::History {
            brand,
            days,
            limit,
            json,
        } => history(&brand, days, limit, json),
        VisibilityAction::Diff { brand, days, json } => diff(&brand, days, json),
        VisibilityAction::Citations {
            brand,
            days,
            limit,
            json,
        } => citations(&brand, days, limit, json),
        VisibilityAction::Providers { json } => providers(json),
    }
}

fn providers(json: bool) -> CmdResult<ExitCode> {
    let rows: Vec<Value> = llm::PROVIDERS
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": p.label,
                "env_key": p.env_key,
                "default_model": p.default_model,
                "live_search": p.live_search,
                "configured": p.configured(),
            })
        })
        .collect();
    let ready = llm::available().len();
    if json {
        print_json(&json!({"providers": rows, "configured": ready}))?;
    } else {
        for p in llm::PROVIDERS {
            println!(
                "{:<12} {:<22} {:<9} {}",
                p.id,
                p.label,
                if p.live_search {
                    "retrieval"
                } else {
                    "knowledge"
                },
                if p.configured() {
                    "ready".to_string()
                } else {
                    format!("set {}", p.env_key)
                }
            );
        }
        eprintln!("{ready}/{} configured", llm::PROVIDERS.len());
    }
    if ready == 0 {
        return Ok(ExitCode::from(1));
    }
    OK
}

/// Scaffold a buyer-intent prompt set.
///
/// Deliberately mechanical. The agent calling this skill is a language model
/// and will write better prompts than a template ever could — this exists so
/// there is always a runnable starting set, and so the JSON shape `run`
/// expects is self-documenting.
const PROMPT_TEMPLATES: &[&str] = &[
    "What is the best {topic} tool?",
    "Best {topic} for a small business",
    "Top {topic} providers compared",
    "What should I look for when choosing {topic}?",
    "Cheapest {topic} option",
    "Most reliable {topic} for enterprise",
    "{topic} alternatives worth considering",
    "Which {topic} has the best reviews?",
    "Recommend a {topic} for a beginner",
    "What are the leading companies in {topic}?",
];

const BRANDED_TEMPLATES: &[&str] = &[
    "Is {brand} a good choice for {topic}?",
    "{brand} reviews — what do users say?",
    "How does {brand} compare to its competitors?",
    "What are the downsides of {brand}?",
];

fn prompts(
    domain: &str,
    brand: Option<&str>,
    topics: &[String],
    count: usize,
    json: bool,
) -> CmdResult<ExitCode> {
    let host = domain
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('/')
        .next()
        .unwrap_or(domain)
        .to_string();
    let inferred_brand = brand
        .map(str::to_string)
        .unwrap_or_else(|| host.split('.').next().unwrap_or(&host).to_string());

    let topics: Vec<String> = if topics.is_empty() {
        vec![format!("{inferred_brand} category")]
    } else {
        topics.to_vec()
    };

    let mut out = Vec::new();
    'outer: for topic in &topics {
        for t in PROMPT_TEMPLATES {
            out.push(t.replace("{topic}", topic));
            if out.len() >= count {
                break 'outer;
            }
        }
        for t in BRANDED_TEMPLATES {
            out.push(
                t.replace("{topic}", topic)
                    .replace("{brand}", &inferred_brand),
            );
            if out.len() >= count {
                break 'outer;
            }
        }
    }

    let doc = json!({
        "brand": inferred_brand,
        "domain": host,
        "topics": topics,
        "prompts": out,
        "note": "A scaffold. Replace these with the questions your buyers actually ask — \
                 the agent running this skill should rewrite them from the site's own \
                 positioning before the first run, because the prompt set is the \
                 measurement instrument.",
    });
    if json {
        print_json(&doc)?;
    } else {
        for p in doc["prompts"].as_array().unwrap_or(&vec![]) {
            println!("{}", p.as_str().unwrap_or(""));
        }
        eprintln!("{} prompts — refine before running", out.len());
    }
    OK
}

pub fn prompts_from(file: Option<&str>, inline: &[String]) -> CmdResult<Vec<String>> {
    let mut out: Vec<String> = inline.to_vec();
    if let Some(path) = file {
        let raw = crate::output::read_source(path)?;
        let trimmed = raw.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            let v: Value = serde_json::from_str(&raw)?;
            let arr = if v.is_array() {
                v
            } else {
                v["prompts"].clone()
            };
            match arr.as_array() {
                Some(items) => {
                    out.extend(items.iter().filter_map(Value::as_str).map(str::to_string))
                }
                None => {
                    return err(format!(
                        "{path}: expected an array, or an object with a \"prompts\" array"
                    ))
                }
            }
        } else {
            // Plain text, one prompt per line — the shape a shell pipeline
            // produces, and the one people hand-edit.
            out.extend(
                raw.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string),
            );
        }
    }
    out.retain(|p| !p.trim().is_empty());
    out.dedup();
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn run(
    brand: &str,
    domain: Option<&str>,
    prompts_file: Option<&str>,
    inline_prompts: &[String],
    provider_spec: Option<&str>,
    model: Option<&str>,
    competitors: &[String],
    aliases: &[String],
    label: Option<&str>,
    concurrency: usize,
    timeout: u64,
    dry_run: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    let prompt_list = prompts_from(prompts_file, inline_prompts)?;
    if prompt_list.is_empty() {
        return err("no prompts — pass --prompts <file> or repeat --prompt, or start from `seogeo visibility prompts`");
    }
    let providers = llm::resolve(provider_spec).map_err(crate::output::Error)?;

    // Every (prompt, provider) pair is one probe. Cost and time both scale
    // with this product, so it is stated up front and `--dry-run` stops here.
    let mut jobs: Vec<(&str, &'static Provider)> = Vec::new();
    for prompt in &prompt_list {
        for p in &providers {
            jobs.push((prompt.as_str(), *p));
        }
    }

    if dry_run {
        let doc = json!({
            "brand": brand,
            "prompts": prompt_list.len(),
            "providers": providers.iter().map(|p| p.id).collect::<Vec<_>>(),
            "probes": jobs.len(),
            "dry_run": true,
        });
        if json {
            print_json(&doc)?;
        } else {
            println!(
                "{} prompts × {} providers = {} probes (nothing sent)",
                prompt_list.len(),
                providers.len(),
                jobs.len()
            );
        }
        return OK;
    }

    if !json {
        eprintln!(
            "{} prompts × {} providers = {} probes",
            prompt_list.len(),
            providers.len(),
            jobs.len()
        );
    }

    let probes = probe_all(&jobs, model, timeout, concurrency.max(1), !json);
    let verdicts: Vec<Verdict> = probes
        .iter()
        .map(|p| judge(p, brand, aliases, domain, competitors))
        .collect();

    let summary = summarise(brand, domain, competitors, &providers, &verdicts);
    let run_id = persist(
        brand,
        domain,
        label,
        &providers,
        competitors,
        &verdicts,
        &summary,
    )?;

    let doc = json!({
        "run_id": run_id,
        "timestamp": now_utc(),
        "brand": brand,
        "domain": domain,
        "label": label,
        "summary": summary,
        "probes": verdicts,
    });

    if json {
        print_json(&doc)?;
    } else {
        print_summary(&summary);
        eprintln!("run #{run_id} stored in {}", db_path().display());
    }
    OK
}

/// Fan the probes out across threads.
///
/// `ureq` is blocking, so this is a plain scoped thread pool over a shared
/// index. Answer engines take tens of seconds each; running a 50-prompt set
/// serially against four providers would take most of an hour.
pub fn probe_all(
    jobs: &[(&str, &'static Provider)],
    model: Option<&str>,
    timeout: u64,
    concurrency: usize,
    progress: bool,
) -> Vec<Probe> {
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<(usize, Probe)>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let workers = concurrency.min(jobs.len()).max(1);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let idx = {
                    let mut guard = next.lock().unwrap_or_else(|e| e.into_inner());
                    let i = *guard;
                    if i >= jobs.len() {
                        return;
                    }
                    *guard += 1;
                    i
                };
                let (prompt, provider) = jobs[idx];
                let probe = llm::probe(provider, model, prompt, timeout);
                if progress {
                    eprintln!(
                        "  [{}/{}] {} — {}",
                        idx + 1,
                        jobs.len(),
                        provider.id,
                        truncate(prompt, 60)
                    );
                }
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((idx, probe));
            });
        }
    });

    let mut out = results.into_inner().unwrap_or_else(|e| e.into_inner());
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, p)| p).collect()
}

fn summarise(
    brand: &str,
    domain: Option<&str>,
    competitors: &[String],
    providers: &[&'static Provider],
    verdicts: &[Verdict],
) -> Value {
    let ok: Vec<&Verdict> = verdicts.iter().filter(|v| v.ok).collect();
    let ok_n = ok.len().max(1) as f64;
    let mentions = ok.iter().filter(|v| v.mentioned).count();
    let mention_rate = mentions as f64 / ok_n;

    let mut competitor_hits: BTreeMap<&str, usize> = BTreeMap::new();
    for c in competitors {
        competitor_hits.insert(c.as_str(), 0);
    }
    for v in &ok {
        for c in &v.competitors_mentioned {
            *competitor_hits.entry(c.as_str()).or_insert(0) += 1;
        }
    }
    let competitor_total: usize = competitor_hits.values().sum();
    let sov = if mentions + competitor_total == 0 {
        0.0
    } else {
        mentions as f64 / (mentions + competitor_total) as f64
    };

    let avg_prominence = if mentions == 0 {
        0.0
    } else {
        ok.iter()
            .filter(|v| v.mentioned)
            .map(|v| v.prominence)
            .sum::<f64>()
            / mentions as f64
    };

    // Citation rate is only meaningful where an engine reports sources.
    let retrieval: Vec<&&Verdict> = ok.iter().filter(|v| v.live_search).collect();
    let citation_rate = if retrieval.is_empty() {
        0.0
    } else {
        retrieval.iter().filter(|v| v.cited_own_domain).count() as f64 / retrieval.len() as f64
    };

    let mut per_provider = serde_json::Map::new();
    for p in providers {
        let subset: Vec<&Verdict> = verdicts
            .iter()
            .filter(|v| v.provider == p.id && v.ok)
            .collect();
        let n = subset.len();
        let m = subset.iter().filter(|v| v.mentioned).count();
        per_provider.insert(
            p.id.to_string(),
            json!({
                "probes": n,
                "mentions": m,
                "mention_rate": rate(m, n),
                "live_search": p.live_search,
                "failed": verdicts.iter().filter(|v| v.provider == p.id && !v.ok).count(),
            }),
        );
    }

    let mut domain_counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in &ok {
        let mut seen: Vec<String> = v
            .citations
            .iter()
            .filter_map(|c| llm::cite_domain(c))
            .collect();
        seen.sort();
        seen.dedup();
        for d in seen {
            *domain_counts.entry(d).or_insert(0) += 1;
        }
    }
    let mut cited: Vec<(String, usize)> = domain_counts.into_iter().collect();
    cited.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let leaderboard: Vec<Value> = std::iter::once(json!({
        "name": brand,
        "mentions": mentions,
        "rate": rate(mentions, ok.len()),
        "is_you": true,
    }))
    .chain(competitor_hits.iter().map(|(name, hits)| {
        json!({"name": name, "mentions": hits, "rate": rate(*hits, ok.len()), "is_you": false})
    }))
    .collect();
    let mut leaderboard = leaderboard;
    leaderboard.sort_by(|a, b| b["mentions"].as_u64().cmp(&a["mentions"].as_u64()));

    json!({
        "brand": brand,
        "domain": domain,
        "probes": verdicts.len(),
        "ok": ok.len(),
        "failed": verdicts.len() - ok.len(),
        "mentions": mentions,
        "mention_rate": round3(mention_rate),
        "share_of_voice": round3(sov),
        "avg_prominence": round3(avg_prominence),
        "citation_rate": round3(citation_rate),
        "retrieval_probes": retrieval.len(),
        "cost_usd": money(verdicts.iter().map(|v| v.cost_usd).sum()),
        "by_provider": per_provider,
        "leaderboard": leaderboard,
        "cited_domains": cited.iter().take(25).map(|(d, n)| json!({"domain": d, "answers": n})).collect::<Vec<_>>(),
        "unanswered_prompts": verdicts
            .iter()
            .filter(|v| v.ok && !v.mentioned)
            .map(|v| v.prompt.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    })
}

fn rate(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        round3(n as f64 / d as f64)
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn persist(
    brand: &str,
    domain: Option<&str>,
    label: Option<&str>,
    providers: &[&'static Provider],
    competitors: &[String],
    verdicts: &[Verdict],
    summary: &Value,
) -> CmdResult<i64> {
    let conn = init_db()?;
    let prompt_count = verdicts
        .iter()
        .map(|v| v.prompt.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    conn.execute(
        "INSERT INTO runs (timestamp, brand, brand_key, domain, label, providers_json,
             competitors_json, prompt_count, probe_count, ok_count, mention_rate,
             share_of_voice, avg_prominence, citation_rate, cost_usd)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![
            now_utc(),
            brand,
            brand_key(brand),
            domain,
            label,
            serde_json::to_string(&providers.iter().map(|p| p.id).collect::<Vec<_>>())?,
            serde_json::to_string(competitors)?,
            prompt_count as i64,
            verdicts.len() as i64,
            summary["ok"].as_u64().unwrap_or(0) as i64,
            summary["mention_rate"].as_f64().unwrap_or(0.0),
            summary["share_of_voice"].as_f64().unwrap_or(0.0),
            summary["avg_prominence"].as_f64().unwrap_or(0.0),
            summary["citation_rate"].as_f64().unwrap_or(0.0),
            summary["cost_usd"].as_f64().unwrap_or(0.0),
        ],
    )?;
    let run_id = conn.last_insert_rowid();

    for v in verdicts {
        conn.execute(
            "INSERT INTO probes (run_id, provider, model, live_search, prompt, ok, mentioned,
                 prominence, cited_own, competitors_json, citations_json, answer, cost_usd, error)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                run_id,
                v.provider,
                v.model,
                v.live_search as i64,
                v.prompt,
                v.ok as i64,
                v.mentioned as i64,
                v.prominence,
                v.cited_own_domain as i64,
                serde_json::to_string(&v.competitors_mentioned)?,
                serde_json::to_string(&v.citations)?,
                // Answers are kept so a later run can diff wording, not just
                // counts — "we dropped out of the top three" is the finding.
                truncate(&v.answer, 8000),
                v.cost_usd,
                v.error,
            ],
        )?;
    }
    Ok(run_id)
}

fn print_summary(s: &Value) {
    let pct = |k: &str| s[k].as_f64().unwrap_or(0.0) * 100.0;
    println!("brand            {}", s["brand"].as_str().unwrap_or(""));
    println!(
        "mention rate     {:.1}%  ({}/{} answers)",
        pct("mention_rate"),
        s["mentions"].as_u64().unwrap_or(0),
        s["ok"].as_u64().unwrap_or(0)
    );
    println!("share of voice   {:.1}%", pct("share_of_voice"));
    println!(
        "avg prominence   {:.2}",
        s["avg_prominence"].as_f64().unwrap_or(0.0)
    );
    let retrieval = s["retrieval_probes"].as_u64().unwrap_or(0);
    if retrieval > 0 {
        println!(
            "own-domain cites {:.1}%  (of {retrieval} retrieval answers)",
            pct("citation_rate")
        );
    } else {
        println!("own-domain cites  n/a — no retrieval engine configured (set PERPLEXITY_API_KEY)");
    }
    if let Some(board) = s["leaderboard"].as_array() {
        println!("\nleaderboard");
        for row in board {
            println!(
                "  {:<28} {:>4}  {:>5.1}%{}",
                row["name"].as_str().unwrap_or(""),
                row["mentions"].as_u64().unwrap_or(0),
                row["rate"].as_f64().unwrap_or(0.0) * 100.0,
                if row["is_you"].as_bool().unwrap_or(false) {
                    "  ← you"
                } else {
                    ""
                }
            );
        }
    }
    if let Some(cites) = s["cited_domains"].as_array() {
        if !cites.is_empty() {
            println!("\nmost-cited sources");
            for row in cites.iter().take(10) {
                println!(
                    "  {:<40} {}",
                    row["domain"].as_str().unwrap_or(""),
                    row["answers"].as_u64().unwrap_or(0)
                );
            }
        }
    }
    let failed = s["failed"].as_u64().unwrap_or(0);
    if failed > 0 {
        eprintln!("\n{failed} probes failed — rerun with --json to see the errors");
    }
}

// ------------------------------------------------------------------ history

struct RunRow {
    id: i64,
    timestamp: String,
    label: Option<String>,
    mention_rate: f64,
    share_of_voice: f64,
    avg_prominence: f64,
    citation_rate: f64,
    probe_count: i64,
    cost_usd: f64,
}

fn load_runs(
    conn: &Connection,
    brand: &str,
    days: Option<i64>,
    limit: usize,
) -> CmdResult<Vec<RunRow>> {
    let since = days.map(crate::output::days_ago);
    let mut stmt = conn.prepare(
        "SELECT id, timestamp, label, mention_rate, share_of_voice, avg_prominence,
                citation_rate, probe_count, cost_usd
         FROM runs
         WHERE brand_key = ?1 AND (?2 IS NULL OR timestamp >= ?2)
         ORDER BY timestamp DESC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![brand_key(brand), since, limit as i64], |r| {
            Ok(RunRow {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                label: r.get(2)?,
                mention_rate: r.get(3)?,
                share_of_voice: r.get(4)?,
                avg_prominence: r.get(5)?,
                citation_rate: r.get(6)?,
                probe_count: r.get(7)?,
                cost_usd: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn history(brand: &str, days: Option<i64>, limit: usize, json: bool) -> CmdResult<ExitCode> {
    let conn = init_db()?;
    let runs = load_runs(&conn, brand, days, limit)?;
    if runs.is_empty() {
        return err(format!(
            "no runs recorded for {brand:?} — start one with `seogeo visibility run --brand {brand}`"
        ));
    }
    let rows: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "run_id": r.id,
                "timestamp": r.timestamp,
                "label": r.label,
                "mention_rate": r.mention_rate,
                "share_of_voice": r.share_of_voice,
                "avg_prominence": r.avg_prominence,
                "citation_rate": r.citation_rate,
                "probes": r.probe_count,
                "cost_usd": r.cost_usd,
            })
        })
        .collect();
    if json {
        print_json(&json!({"brand": brand, "runs": rows, "count": rows.len()}))?;
    } else {
        println!(
            "{:<20} {:>8} {:>8} {:>8} {:>7}  label",
            "when", "mention", "SoV", "cites", "probes"
        );
        for r in &runs {
            println!(
                "{:<20} {:>7.1}% {:>7.1}% {:>7.1}% {:>7}  {}",
                &r.timestamp[..r.timestamp.len().min(19)],
                r.mention_rate * 100.0,
                r.share_of_voice * 100.0,
                r.citation_rate * 100.0,
                r.probe_count,
                r.label.clone().unwrap_or_default()
            );
        }
        println!(
            "\n{}",
            sparkline(
                &runs
                    .iter()
                    .rev()
                    .map(|r| r.mention_rate)
                    .collect::<Vec<_>>()
            )
        );
    }
    OK
}

/// Eight-level block sparkline. Terminal output is where most of these runs
/// are read, and a trend is the whole point.
fn sparkline(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().cloned().fold(f64::MIN, f64::max).max(0.0001);
    let line: String = values
        .iter()
        .map(|v| BARS[(((v / max) * 7.0).round() as usize).min(7)])
        .collect();
    format!(
        "mention rate {line} (oldest → newest, peak {:.1}%)",
        max * 100.0
    )
}

fn diff(brand: &str, days: i64, json: bool) -> CmdResult<ExitCode> {
    let conn = init_db()?;
    let all = load_runs(&conn, brand, None, 500)?;
    if all.len() < 2 {
        return err(format!(
            "need at least two runs for {brand:?} — {} recorded",
            all.len()
        ));
    }
    let cutoff = crate::output::days_ago(days);
    let latest = &all[0];
    // The comparison point is the newest run that predates the window, so
    // `--days 30` answers "versus a month ago" rather than "versus whichever
    // run happens to be second in the list".
    let baseline = all
        .iter()
        .skip(1)
        .find(|r| r.timestamp < cutoff)
        .unwrap_or(&all[all.len() - 1]);

    let delta = |now: f64, then: f64| {
        json!({
            "now": round3(now),
            "then": round3(then),
            "delta": round3(now - then),
            "delta_pct_points": round3((now - then) * 100.0),
        })
    };

    let doc = json!({
        "brand": brand,
        "window_days": days,
        "latest": {"run_id": latest.id, "timestamp": latest.timestamp, "label": latest.label},
        "baseline": {"run_id": baseline.id, "timestamp": baseline.timestamp, "label": baseline.label},
        "mention_rate": delta(latest.mention_rate, baseline.mention_rate),
        "share_of_voice": delta(latest.share_of_voice, baseline.share_of_voice),
        "avg_prominence": delta(latest.avg_prominence, baseline.avg_prominence),
        "citation_rate": delta(latest.citation_rate, baseline.citation_rate),
        "regressed": latest.mention_rate < baseline.mention_rate
            || latest.share_of_voice < baseline.share_of_voice,
    });

    if json {
        print_json(&doc)?;
    } else {
        println!("{brand}: run #{} vs #{}", latest.id, baseline.id);
        for key in [
            "mention_rate",
            "share_of_voice",
            "avg_prominence",
            "citation_rate",
        ] {
            let d = &doc[key];
            let (then, now) = (
                d["then"].as_f64().unwrap_or(0.0),
                d["now"].as_f64().unwrap_or(0.0),
            );
            // Prominence is a 0–1 position score, not a proportion of
            // anything; rendering it as a percentage invites it to be read
            // as "57% of answers", which is a different claim entirely.
            if key == "avg_prominence" {
                let delta = now - then;
                println!(
                    "  {key:<16} {then:>6.2}  → {now:>6.2}    {}{:.2}",
                    if delta >= 0.0 { "+" } else { "" },
                    delta
                );
                continue;
            }
            let pts = d["delta_pct_points"].as_f64().unwrap_or(0.0);
            println!(
                "  {key:<16} {:>6.1}% → {:>6.1}%   {}{:.1} pts",
                then * 100.0,
                now * 100.0,
                if pts >= 0.0 { "+" } else { "" },
                pts
            );
        }
    }
    // A regression is an exit code so `seogeo watch` and CI can act on it
    // without parsing the report.
    if doc["regressed"].as_bool().unwrap_or(false) {
        return Ok(ExitCode::from(2));
    }
    OK
}

fn citations(brand: &str, days: Option<i64>, limit: usize, json: bool) -> CmdResult<ExitCode> {
    let conn = init_db()?;
    let since = days.map(crate::output::days_ago);
    let mut stmt = conn.prepare(
        "SELECT p.citations_json FROM probes p
         JOIN runs r ON r.id = p.run_id
         WHERE r.brand_key = ?1 AND p.ok = 1 AND (?2 IS NULL OR r.timestamp >= ?2)",
    )?;
    let lists = stmt
        .query_map(params![brand_key(brand), since], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut answers = 0usize;
    for raw in &lists {
        let urls: Vec<String> = serde_json::from_str(raw).unwrap_or_default();
        if urls.is_empty() {
            continue;
        }
        answers += 1;
        let mut seen: Vec<String> = urls.iter().filter_map(|u| llm::cite_domain(u)).collect();
        seen.sort();
        seen.dedup();
        for d in seen {
            *counts.entry(d).or_insert(0) += 1;
        }
    }
    if answers == 0 {
        return err(format!(
            "no cited sources recorded for {brand:?} — only retrieval engines return them (set PERPLEXITY_API_KEY)"
        ));
    }
    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rows.truncate(limit);

    let out: Vec<Value> = rows
        .iter()
        .map(|(d, n)| json!({"domain": d, "answers": n, "share": rate(*n, answers)}))
        .collect();
    if json {
        print_json(&json!({"brand": brand, "answers_with_sources": answers, "domains": out}))?;
    } else {
        println!("{answers} answers carried sources");
        for (d, n) in &rows {
            println!(
                "  {:<44} {:>4}  {:>5.1}%",
                d,
                n,
                *n as f64 / answers as f64 * 100.0
            );
        }
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_requires_word_boundaries() {
        assert!(find_mention("We recommend Notion for this.", "Notion").found);
        assert!(!find_mention("Speaking notionally about it.", "Notion").found);
        assert!(find_mention("Try Notion's database.", "Notion").found);
    }

    #[test]
    fn prominence_is_higher_when_named_early() {
        let early = find_mention(
            "Acme is the leader. Then a long tail of other options follows here.",
            "Acme",
        );
        let late = find_mention(
            "Many options exist and are worth a look, and finally Acme.",
            "Acme",
        );
        assert!(early.found && late.found);
        assert!(early.prominence > late.prominence);
    }

    #[test]
    fn missing_brand_scores_zero() {
        let m = find_mention("No such vendor here.", "Acme");
        assert!(!m.found);
        assert_eq!(m.prominence, 0.0);
    }

    #[test]
    fn prompt_file_accepts_json_and_plain_text() {
        let dir = std::env::temp_dir().join("seogeo-vis-test");
        std::fs::create_dir_all(&dir).unwrap();
        let j = dir.join("p.json");
        std::fs::write(&j, r#"{"prompts":["a","b"]}"#).unwrap();
        assert_eq!(
            prompts_from(Some(j.to_str().unwrap()), &[]).unwrap(),
            vec!["a", "b"]
        );

        let t = dir.join("p.txt");
        std::fs::write(&t, "# comment\nfirst\n\nsecond\n").unwrap();
        assert_eq!(
            prompts_from(Some(t.to_str().unwrap()), &[]).unwrap(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn sparkline_handles_flat_and_empty() {
        assert_eq!(sparkline(&[]), "");
        assert!(sparkline(&[0.5, 0.5]).contains("██"));
    }
}

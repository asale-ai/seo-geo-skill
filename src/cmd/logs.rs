//! AI crawler log analysis.
//!
//! Every other check in this binary probes a site from the outside and
//! infers. Access logs are the only first-party record of what the answer
//! engines actually did: which bot came, what it took, what it was refused,
//! and — through the referrer — what it sent back.
//!
//! That data is also why this belongs in a local binary rather than a hosted
//! dashboard. Access logs carry visitor IPs; uploading them to a SaaS is a
//! privacy decision, and parsing them on the operator's own machine is not.
//! Nothing here leaves the process unless `--sitemap` is passed.
//!
//! The distinction the report is built around: a *training* crawl can never
//! produce a citation, a *search* crawl can, and a *user-action* fetch means
//! a human in an assistant asked for that page right then. Lumping them into
//! one "AI traffic" number hides the only part that converts.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::output::{err, print_json, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

/// What a crawler is for. Purpose decides whether a crawl can ever pay off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    /// Corpus collection for model training. No citation can follow.
    Training,
    /// Index building for an answer engine. A citation can follow.
    Search,
    /// A person in an assistant asked for this page right now.
    UserAction,
    /// Declared but unclear, or a general-purpose fetcher.
    Other,
}

pub struct Bot {
    /// Substring matched case-insensitively against the User-Agent.
    pub token: &'static str,
    pub platform: &'static str,
    pub purpose: Purpose,
}

/// User-Agent tokens, longest-first within a platform so `ChatGPT-User` is
/// not swallowed by a looser `GPTBot` match.
pub const BOTS: &[Bot] = &[
    Bot {
        token: "OAI-SearchBot",
        platform: "OpenAI",
        purpose: Purpose::Search,
    },
    Bot {
        token: "ChatGPT-User",
        platform: "OpenAI",
        purpose: Purpose::UserAction,
    },
    Bot {
        token: "GPTBot",
        platform: "OpenAI",
        purpose: Purpose::Training,
    },
    Bot {
        token: "Claude-SearchBot",
        platform: "Anthropic",
        purpose: Purpose::Search,
    },
    Bot {
        token: "Claude-User",
        platform: "Anthropic",
        purpose: Purpose::UserAction,
    },
    Bot {
        token: "ClaudeBot",
        platform: "Anthropic",
        purpose: Purpose::Training,
    },
    Bot {
        token: "anthropic-ai",
        platform: "Anthropic",
        purpose: Purpose::Training,
    },
    Bot {
        token: "PerplexityBot",
        platform: "Perplexity",
        purpose: Purpose::Search,
    },
    Bot {
        token: "Perplexity-User",
        platform: "Perplexity",
        purpose: Purpose::UserAction,
    },
    Bot {
        token: "Google-Extended",
        platform: "Google",
        purpose: Purpose::Training,
    },
    Bot {
        token: "GoogleOther",
        platform: "Google",
        purpose: Purpose::Other,
    },
    Bot {
        token: "Googlebot",
        platform: "Google",
        purpose: Purpose::Search,
    },
    Bot {
        token: "bingbot",
        platform: "Microsoft",
        purpose: Purpose::Search,
    },
    Bot {
        token: "BingPreview",
        platform: "Microsoft",
        purpose: Purpose::Other,
    },
    Bot {
        token: "CCBot",
        platform: "Common Crawl",
        purpose: Purpose::Training,
    },
    Bot {
        token: "Bytespider",
        platform: "ByteDance",
        purpose: Purpose::Training,
    },
    Bot {
        token: "Amazonbot",
        platform: "Amazon",
        purpose: Purpose::Other,
    },
    Bot {
        token: "Applebot-Extended",
        platform: "Apple",
        purpose: Purpose::Training,
    },
    Bot {
        token: "Applebot",
        platform: "Apple",
        purpose: Purpose::Search,
    },
    Bot {
        token: "meta-externalagent",
        platform: "Meta",
        purpose: Purpose::Training,
    },
    Bot {
        token: "FacebookBot",
        platform: "Meta",
        purpose: Purpose::Training,
    },
    Bot {
        token: "MistralAI-User",
        platform: "Mistral",
        purpose: Purpose::UserAction,
    },
    Bot {
        token: "cohere-ai",
        platform: "Cohere",
        purpose: Purpose::Training,
    },
    Bot {
        token: "DuckAssistBot",
        platform: "DuckDuckGo",
        purpose: Purpose::Search,
    },
    Bot {
        token: "YouBot",
        platform: "You.com",
        purpose: Purpose::Search,
    },
];

/// Referrer hosts that mean a human arrived from an assistant.
const REFERRERS: &[(&str, &str)] = &[
    ("chatgpt.com", "OpenAI"),
    ("chat.openai.com", "OpenAI"),
    ("perplexity.ai", "Perplexity"),
    ("claude.ai", "Anthropic"),
    ("gemini.google.com", "Google"),
    ("bard.google.com", "Google"),
    ("copilot.microsoft.com", "Microsoft"),
    ("edgeservices.bing.com", "Microsoft"),
    ("you.com", "You.com"),
    ("phind.com", "Phind"),
    ("grok.com", "xAI"),
    ("chat.mistral.ai", "Mistral"),
    ("duckduckgo.com", "DuckDuckGo"),
];

pub fn classify(user_agent: &str) -> Option<&'static Bot> {
    let ua = user_agent.to_ascii_lowercase();
    BOTS.iter()
        .find(|b| ua.contains(&b.token.to_ascii_lowercase()))
}

fn referrer_platform(referrer: &str) -> Option<&'static str> {
    let r = referrer.to_ascii_lowercase();
    REFERRERS
        .iter()
        .find(|(host, _)| r.contains(host))
        .map(|(_, platform)| *platform)
}

// ------------------------------------------------------------------ parsing

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub path: String,
    pub status: u16,
    pub user_agent: String,
    pub referrer: String,
    pub bytes: u64,
    /// `YYYY-MM-DD`, empty when the line carried no parseable date.
    pub date: String,
}

/// Parse one line of NCSA combined format.
///
/// Written by hand rather than with a regex because the quoted fields make a
/// correct regex slower and harder to read than a two-pass scan, and this
/// runs over millions of lines.
pub fn parse_combined(line: &str) -> Option<Entry> {
    let bracket_open = line.find('[')?;
    let bracket_close = line[bracket_open..].find(']')? + bracket_open;
    let date = parse_clf_date(&line[bracket_open + 1..bracket_close]);

    // Quoted segments, in order: request, referrer, user-agent.
    let mut quoted = Vec::new();
    let rest = &line[bracket_close + 1..];
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    buf.push(chars[j + 1]);
                    j += 2;
                    continue;
                }
                if chars[j] == '"' {
                    break;
                }
                buf.push(chars[j]);
                j += 1;
            }
            quoted.push(buf);
            i = j + 1;
            continue;
        }
        i += 1;
    }
    if quoted.is_empty() {
        return None;
    }

    let request = &quoted[0];
    let path = request.split_whitespace().nth(1).unwrap_or("").to_string();

    // Status and size sit between the request quote and the referrer quote.
    let after_request = rest.splitn(3, '"').nth(2).unwrap_or("");
    let mut nums = after_request.split_whitespace();
    let status = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let bytes = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    Some(Entry {
        path,
        status,
        referrer: quoted.get(1).cloned().unwrap_or_default(),
        user_agent: quoted.get(2).cloned().unwrap_or_default(),
        bytes,
        date,
    })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `10/Oct/2025:13:55:36 -0700` → `2025-10-10`.
fn parse_clf_date(raw: &str) -> String {
    let head = raw.split(':').next().unwrap_or("");
    let mut parts = head.split('/');
    let (Some(d), Some(m), Some(y)) = (parts.next(), parts.next(), parts.next()) else {
        return String::new();
    };
    let Some(mi) = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(m)) else {
        return String::new();
    };
    let (Ok(dn), Ok(yn)) = (d.parse::<u32>(), y.parse::<u32>()) else {
        return String::new();
    };
    format!("{yn:04}-{:02}-{dn:02}", mi + 1)
}

/// Parse a JSON log line, tolerating the field names Cloudflare Logpush,
/// Vercel, and most structured nginx setups use.
pub fn parse_json_line(line: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(line).ok()?;
    let pick = |keys: &[&str]| -> String {
        for k in keys {
            if let Some(s) = v.get(*k).and_then(Value::as_str) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        String::new()
    };
    let status = [
        "EdgeResponseStatus",
        "status",
        "statusCode",
        "response_status",
    ]
    .iter()
    .find_map(|k| v.get(*k).and_then(Value::as_u64))
    .unwrap_or(0) as u16;

    let raw_path = pick(&[
        "ClientRequestURI",
        "ClientRequestPath",
        "path",
        "url",
        "request_uri",
    ]);
    if raw_path.is_empty() {
        return None;
    }
    let ts = pick(&[
        "EdgeStartTimestamp",
        "timestamp",
        "time",
        "@timestamp",
        "datetime",
    ]);
    let date = ts
        .split('T')
        .next()
        .unwrap_or("")
        .chars()
        .take(10)
        .collect::<String>();

    Some(Entry {
        path: raw_path,
        status,
        user_agent: pick(&[
            "ClientRequestUserAgent",
            "user_agent",
            "userAgent",
            "http_user_agent",
        ]),
        referrer: pick(&[
            "ClientRequestReferer",
            "referer",
            "referrer",
            "http_referer",
        ]),
        bytes: ["EdgeResponseBytes", "bytes", "body_bytes_sent"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_u64))
            .unwrap_or(0),
        date: if date.len() == 10 {
            date
        } else {
            String::new()
        },
    })
}

fn open_lines(path: &str) -> CmdResult<Box<dyn BufRead>> {
    if path == "-" {
        return Ok(Box::new(std::io::BufReader::new(std::io::stdin())));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| crate::output::Error(format!("could not read {path}: {e}")))?;
    if path.ends_with(".gz") {
        // Rotated logs arrive gzipped far more often than not.
        Ok(Box::new(std::io::BufReader::new(
            flate2::read::GzDecoder::new(file),
        )))
    } else {
        Ok(Box::new(std::io::BufReader::new(file)))
    }
}

// ---------------------------------------------------------------- accounting

#[derive(Default, Clone)]
struct Tally {
    hits: u64,
    bytes: u64,
    blocked: u64,
    rate_limited: u64,
    not_found: u64,
    paths: BTreeMap<String, u64>,
    statuses: BTreeMap<u16, u64>,
}

impl Tally {
    fn record(&mut self, e: &Entry) {
        self.hits += 1;
        self.bytes += e.bytes;
        match e.status {
            401 | 403 => self.blocked += 1,
            429 => self.rate_limited += 1,
            404 | 410 => self.not_found += 1,
            _ => {}
        }
        *self.statuses.entry(e.status).or_insert(0) += 1;
        // Cap the path table so a log full of unique query strings cannot
        // grow it without bound; the top-N report only ever reads the head.
        if self.paths.len() < 50_000 {
            *self.paths.entry(strip_query(&e.path)).or_insert(0) += 1;
        }
    }
}

fn strip_query(path: &str) -> String {
    path.split(['?', '#']).next().unwrap_or(path).to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn analyze(
    files: &[String],
    sitemap: Option<&str>,
    since: Option<&str>,
    top: usize,
    include_all_bots: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    if files.is_empty() {
        return err("pass one or more log files, or `-` for stdin");
    }

    let mut total_lines = 0u64;
    let mut unparsed = 0u64;
    let mut human_hits = 0u64;
    let mut by_bot: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut by_platform: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut purposes: BTreeMap<Purpose, u64> = BTreeMap::new();
    let mut referrals: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut referral_paths: BTreeMap<String, u64> = BTreeMap::new();
    let mut crawled_paths: std::collections::BTreeSet<String> = Default::default();
    let mut first_date = String::new();
    let mut last_date = String::new();

    for path in files {
        let reader = open_lines(path)?;
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            total_lines += 1;
            let entry = if line.starts_with('{') {
                parse_json_line(line)
            } else {
                parse_combined(line)
            };
            let Some(e) = entry else {
                unparsed += 1;
                continue;
            };

            if let Some(cut) = since {
                if !e.date.is_empty() && e.date.as_str() < cut {
                    continue;
                }
            }
            if !e.date.is_empty() {
                if first_date.is_empty() || e.date < first_date {
                    first_date = e.date.clone();
                }
                if e.date > last_date {
                    last_date = e.date.clone();
                }
            }

            // A referral is a human arriving from an assistant, so it is
            // counted regardless of which UA carried the request.
            if let Some(platform) = referrer_platform(&e.referrer) {
                *referrals.entry(platform).or_insert(0) += 1;
                *referral_paths.entry(strip_query(&e.path)).or_insert(0) += 1;
            }

            match classify(&e.user_agent) {
                Some(bot) => {
                    by_bot.entry(bot.token).or_default().record(&e);
                    by_platform.entry(bot.platform).or_default().record(&e);
                    *purposes.entry(bot.purpose).or_insert(0) += 1;
                    if e.status < 400 && crawled_paths.len() < 200_000 {
                        crawled_paths.insert(strip_query(&e.path));
                    }
                }
                None => human_hits += 1,
            }
        }
    }

    if total_lines == 0 {
        return err("no log lines read");
    }

    let bot_hits: u64 = by_bot.values().map(|t| t.hits).sum();
    let total_referrals: u64 = referrals.values().sum();

    // The headline number. A crawler that takes thousands of pages per
    // referral is extracting, not distributing; the ratio is what makes that
    // legible, and it is the metric the 2026 reporting settled on.
    let platform_rows: Vec<Value> = by_platform
        .iter()
        .map(|(platform, t)| {
            let refs = referrals.get(platform).copied().unwrap_or(0);
            json!({
                "platform": platform,
                "crawls": t.hits,
                "referrals": refs,
                "crawl_to_referral": if refs == 0 { Value::Null } else { json!(round1(t.hits as f64 / refs as f64)) },
                "bytes": t.bytes,
                "blocked_403": t.blocked,
                "rate_limited_429": t.rate_limited,
                "not_found": t.not_found,
            })
        })
        .collect();

    let bot_rows: Vec<Value> = by_bot
        .iter()
        .map(|(token, t)| {
            let bot = BOTS.iter().find(|b| b.token == *token).expect("token came from BOTS");
            json!({
                "bot": token,
                "platform": bot.platform,
                "purpose": bot.purpose,
                "hits": t.hits,
                "bytes": t.bytes,
                "blocked_403": t.blocked,
                "rate_limited_429": t.rate_limited,
                "not_found": t.not_found,
                "block_rate": ratio(t.blocked + t.rate_limited, t.hits),
                "top_paths": top_n(&t.paths, top.min(10)),
                "statuses": t.statuses.iter().map(|(k, v)| json!({"status": k, "hits": v})).collect::<Vec<_>>(),
            })
        })
        .collect();

    let search_hits = purposes.get(&Purpose::Search).copied().unwrap_or(0);
    let training_hits = purposes.get(&Purpose::Training).copied().unwrap_or(0);
    let user_hits = purposes.get(&Purpose::UserAction).copied().unwrap_or(0);

    // Findings are the part an agent acts on, so they are computed here
    // rather than left for the skill to infer from the counts.
    let mut findings = Vec::new();
    let blocked_total: u64 = by_bot.values().map(|t| t.blocked + t.rate_limited).sum();
    if blocked_total > 0 {
        let worst: Vec<String> = by_bot
            .iter()
            .filter(|(_, t)| t.blocked + t.rate_limited > 0)
            .map(|(k, t)| format!("{k} ({} of {})", t.blocked + t.rate_limited, t.hits))
            .collect();
        findings.push(json!({
            "severity": if search_purpose_blocked(&by_bot) { "critical" } else { "warning" },
            "finding": "AI crawlers are being refused",
            "detail": format!("{blocked_total} requests returned 403/429 — {}", worst.join(", ")),
            "why": "A search-purpose crawler that cannot fetch the page cannot cite it. \
                    Blocks are usually a WAF or bot-management rule, not robots.txt.",
        }));
    }
    if search_hits == 0 && bot_hits > 0 {
        findings.push(json!({
            "severity": "critical",
            "finding": "No search-purpose crawling at all",
            "detail": format!("{bot_hits} AI crawler requests, none from a search-purpose bot"),
            "why": "Only search crawlers build the index an answer can cite. Training-only \
                    crawling takes the content without ever being able to return traffic.",
        }));
    }
    if training_hits > 0 && search_hits > 0 && training_hits > search_hits * 5 {
        findings.push(json!({
            "severity": "info",
            "finding": "Crawling is overwhelmingly for training",
            "detail": format!("{training_hits} training vs {search_hits} search requests"),
            "why": "Expected in 2026 — but it means most of the bandwidth spent on AI bots \
                    can never produce a citation. Worth knowing before optimising for it.",
        }));
    }
    if total_referrals == 0 && bot_hits > 0 {
        findings.push(json!({
            "severity": "warning",
            "finding": "No assistant referrals recorded",
            "detail": "Crawlers arrive, no humans follow",
            "why": "Either the answer engines are not citing you, or the referrer is being \
                    stripped before it reaches these logs (check your CDN and any redirect hop).",
        }));
    }
    if user_hits > 0 {
        findings.push(json!({
            "severity": "good",
            "finding": "Live user-action fetches present",
            "detail": format!("{user_hits} requests from assistant user-agents acting for a person"),
            "why": "Someone in an assistant asked for your page in real time. This is the \
                    crawl class most closely tied to an actual answer being built about you.",
        }));
    }

    let mut doc = json!({
        "files": files,
        "lines_read": total_lines,
        "lines_unparsed": unparsed,
        "date_range": {"from": first_date, "to": last_date},
        "traffic": {
            "ai_crawler_requests": bot_hits,
            "other_requests": human_hits,
            "ai_share": ratio(bot_hits, bot_hits + human_hits),
        },
        "by_purpose": {
            "search": search_hits,
            "training": training_hits,
            "user_action": user_hits,
            "other": purposes.get(&Purpose::Other).copied().unwrap_or(0),
        },
        "platforms": platform_rows,
        "bots": bot_rows,
        "referrals": {
            "total": total_referrals,
            "by_platform": referrals.iter().map(|(k, v)| json!({"platform": k, "visits": v})).collect::<Vec<_>>(),
            "top_landing_pages": top_n(&referral_paths, top),
        },
        "findings": findings,
    });

    if include_all_bots {
        doc["note_unmatched"] = json!(
            "other_requests counts every user-agent this build does not recognise as an AI \
             crawler, humans and ordinary bots alike — it is not a human-visitor count."
        );
    }

    if let Some(sm) = sitemap {
        doc["coverage"] = coverage(sm, &crawled_paths, top)?;
    }

    if json {
        print_json(&doc)?;
    } else {
        print_human(&doc);
    }

    let critical = doc["findings"]
        .as_array()
        .map(|f| f.iter().any(|x| x["severity"] == "critical"))
        .unwrap_or(false);
    if critical {
        return Ok(ExitCode::from(2));
    }
    OK
}

fn search_purpose_blocked(by_bot: &BTreeMap<&'static str, Tally>) -> bool {
    by_bot.iter().any(|(token, t)| {
        (t.blocked + t.rate_limited) > 0
            && BOTS.iter().any(|b| {
                b.token == *token && matches!(b.purpose, Purpose::Search | Purpose::UserAction)
            })
    })
}

/// Which sitemap URLs no AI crawler has ever successfully fetched.
///
/// This is the question a robots.txt audit cannot answer: permission granted
/// is not the same as page retrieved.
fn coverage(
    sitemap: &str,
    crawled: &std::collections::BTreeSet<String>,
    top: usize,
) -> CmdResult<Value> {
    let urls = crate::cmd::core::sitemap_urls_for(sitemap, 5000);
    if urls.is_empty() {
        return Ok(json!({"error": format!("no URLs found at {sitemap}")}));
    }
    let mut missed = Vec::new();
    let mut hit = 0usize;
    for u in &urls {
        let path = url::Url::parse(u)
            .ok()
            .map(|p| p.path().to_string())
            .unwrap_or_else(|| u.clone());
        if crawled.contains(&path) || crawled.contains(path.trim_end_matches('/')) {
            hit += 1;
        } else {
            missed.push(path);
        }
    }
    let total = urls.len();
    missed.truncate(top.max(1));
    Ok(json!({
        "sitemap": sitemap,
        "sitemap_urls": total,
        "fetched_by_ai_crawlers": hit,
        "coverage": ratio(hit as u64, total as u64),
        "never_fetched_sample": missed,
    }))
}

fn top_n(map: &BTreeMap<String, u64>, n: usize) -> Vec<Value> {
    let mut rows: Vec<(&String, &u64)> = map.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    rows.into_iter()
        .take(n)
        .map(|(p, c)| json!({"path": p, "hits": c}))
        .collect()
}

fn ratio(n: u64, d: u64) -> f64 {
    if d == 0 {
        0.0
    } else {
        ((n as f64 / d as f64) * 1000.0).round() / 1000.0
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn print_human(doc: &Value) {
    let t = &doc["traffic"];
    println!(
        "{} lines · {} AI crawler requests ({:.1}% of traffic) · {} → {}",
        doc["lines_read"].as_u64().unwrap_or(0),
        t["ai_crawler_requests"].as_u64().unwrap_or(0),
        t["ai_share"].as_f64().unwrap_or(0.0) * 100.0,
        doc["date_range"]["from"].as_str().unwrap_or("?"),
        doc["date_range"]["to"].as_str().unwrap_or("?"),
    );
    let p = &doc["by_purpose"];
    println!(
        "purpose: {} search · {} training · {} user-action",
        p["search"].as_u64().unwrap_or(0),
        p["training"].as_u64().unwrap_or(0),
        p["user_action"].as_u64().unwrap_or(0),
    );

    println!(
        "\n{:<14} {:>9} {:>10} {:>14} {:>8}",
        "platform", "crawls", "referrals", "crawl:referral", "blocked"
    );
    for row in doc["platforms"].as_array().unwrap_or(&vec![]) {
        let ratio = match row["crawl_to_referral"].as_f64() {
            Some(r) => format!("{r:.1}:1"),
            None => "—".to_string(),
        };
        println!(
            "{:<14} {:>9} {:>10} {:>14} {:>8}",
            row["platform"].as_str().unwrap_or(""),
            row["crawls"].as_u64().unwrap_or(0),
            row["referrals"].as_u64().unwrap_or(0),
            ratio,
            row["blocked_403"].as_u64().unwrap_or(0)
                + row["rate_limited_429"].as_u64().unwrap_or(0),
        );
    }

    if let Some(cov) = doc.get("coverage") {
        if let Some(total) = cov["sitemap_urls"].as_u64() {
            println!(
                "\ncoverage: {}/{} sitemap URLs fetched by an AI crawler ({:.1}%)",
                cov["fetched_by_ai_crawlers"].as_u64().unwrap_or(0),
                total,
                cov["coverage"].as_f64().unwrap_or(0.0) * 100.0
            );
            for p in cov["never_fetched_sample"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .take(10)
            {
                println!("  never fetched: {}", p.as_str().unwrap_or(""));
            }
        }
    }

    println!();
    for f in doc["findings"].as_array().unwrap_or(&vec![]) {
        println!(
            "[{}] {} — {}",
            f["severity"].as_str().unwrap_or(""),
            f["finding"].as_str().unwrap_or(""),
            f["detail"].as_str().unwrap_or("")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"66.249.66.1 - - [10/Oct/2025:13:55:36 -0700] "GET /pricing?utm=x HTTP/1.1" 200 2326 "https://chatgpt.com/" "Mozilla/5.0 (compatible; GPTBot/1.2; +https://openai.com/gptbot)""#;

    #[test]
    fn parses_combined_line() {
        let e = parse_combined(LINE).expect("parses");
        assert_eq!(e.path, "/pricing?utm=x");
        assert_eq!(e.status, 200);
        assert_eq!(e.bytes, 2326);
        assert_eq!(e.date, "2025-10-10");
        assert!(e.user_agent.contains("GPTBot"));
        assert_eq!(e.referrer, "https://chatgpt.com/");
    }

    #[test]
    fn classifies_user_action_before_training() {
        assert_eq!(
            classify("ChatGPT-User/1.0").unwrap().purpose,
            Purpose::UserAction
        );
        assert_eq!(classify("GPTBot/1.2").unwrap().purpose, Purpose::Training);
        assert_eq!(
            classify("OAI-SearchBot/1.0").unwrap().purpose,
            Purpose::Search
        );
        assert!(classify("Mozilla/5.0 (Macintosh)").is_none());
    }

    #[test]
    fn parses_cloudflare_json() {
        let line = r#"{"ClientRequestURI":"/a","EdgeResponseStatus":403,"ClientRequestUserAgent":"ClaudeBot/1.0","ClientRequestReferer":"","EdgeStartTimestamp":"2026-03-04T10:00:00Z"}"#;
        let e = parse_json_line(line).expect("parses");
        assert_eq!(e.status, 403);
        assert_eq!(e.date, "2026-03-04");
        assert_eq!(classify(&e.user_agent).unwrap().platform, "Anthropic");
    }

    #[test]
    fn referrer_maps_to_platform() {
        assert_eq!(
            referrer_platform("https://www.perplexity.ai/search/x"),
            Some("Perplexity")
        );
        assert_eq!(referrer_platform("https://example.com/"), None);
    }

    #[test]
    fn query_strings_collapse_into_one_path() {
        assert_eq!(strip_query("/a?b=1#c"), "/a");
    }

    #[test]
    fn malformed_lines_do_not_panic() {
        assert!(parse_combined("").is_none());
        assert!(parse_combined("garbage without brackets").is_none());
        assert!(parse_json_line("{}").is_none());
    }
}

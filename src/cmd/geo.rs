//! GEO commands: passage-level citability scoring and brand-authority scanning.

use std::process::ExitCode;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::json;

use crate::cmd::core::fetch_record;
use crate::html;
use crate::http::{self, RequestOptions};
use crate::output::{err, print_json, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

// -------------------------------------------------------------- citability

#[derive(Serialize, Clone, Debug)]
pub struct Breakdown {
    pub answer_block_quality: i64,
    pub self_containment: i64,
    pub structural_readability: i64,
    pub statistical_density: i64,
    pub uniqueness_signals: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct PassageScore {
    pub heading: Option<String>,
    pub word_count: usize,
    pub total_score: i64,
    pub grade: &'static str,
    pub label: &'static str,
    pub breakdown: Breakdown,
    pub preview: String,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

macro_rules! lazy_re {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Regex {
            static CELL: OnceLock<Regex> = OnceLock::new();
            CELL.get_or_init(|| re($pattern))
        }
    };
}

lazy_re!(
    re_definition,
    r"(?i)\b\w+\s+is\s+(?:a|an|the)\s|\b\w+\s+refers?\s+to\s|\b\w+\s+means?\s|\b\w+\s+(?:can be |are )?defined\s+as\s|\bin\s+(?:simple|other)\s+(?:terms|words)\s*,"
);
lazy_re!(
    re_early_answer,
    r"(?i)\b(?:is|are|was|were|means?|refers?)\b|\d+%|\$[\d,]+|\d+\s+(?:million|billion|thousand)"
);
lazy_re!(
    re_research,
    r"(?i)(?:according to|research shows|studies? (?:show|indicate|suggest|found)|data (?:shows|indicates|suggests))"
);
lazy_re!(
    re_pronouns,
    r"(?i)\b(?:it|they|them|their|this|that|these|those|he|she|his|her)\b"
);
lazy_re!(re_proper_nouns, r"\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\b");
lazy_re!(
    re_sequence,
    r"(?i)(?:first|second|third|finally|additionally|moreover|furthermore)"
);
lazy_re!(
    re_enumerated,
    r"(?i)(?:\d+[.)]\s|\b(?:step|tip|point)\s+\d+)"
);
lazy_re!(re_percent, r"\d+(?:\.\d+)?%");
lazy_re!(
    re_dollar,
    r"(?i)\$[\d,]+(?:\.\d+)?(?:\s*(?:million|billion|M|B|K))?"
);
lazy_re!(
    re_counted_nouns,
    r"(?i)\b\d+(?:,\d{3})*(?:\.\d+)?\s+(?:users|customers|pages|sites|companies|businesses|people|percent|times|x\b)"
);
lazy_re!(re_recent_year, r"\b20(?:2[3-9]|3\d|1\d)\b");
lazy_re!(re_source_attr, r"(?:according to|per|from|by)\s+[A-Z]");
lazy_re!(
    re_source_org,
    r"(?:Gartner|Forrester|McKinsey|Harvard|Stanford|MIT|Google|Microsoft|OpenAI|Anthropic)"
);
lazy_re!(re_source_paren, r"\([A-Z][a-z]+(?:\s+\d{4})?\)");
lazy_re!(
    re_original_data,
    r"(?i)(?:our (?:research|study|data|analysis|survey|findings)|we (?:found|discovered|analyzed|surveyed|measured))"
);
lazy_re!(
    re_case_study,
    r"(?i)(?:case study|for example|for instance|in practice|real-world|hands-on)"
);
lazy_re!(re_tool_mention, r"(?:using|with|via|through)\s+[A-Z][a-z]+");

/// Score one passage 0-100 for how likely an answer engine is to quote it.
///
/// Weighting follows the published analysis of AI-cited passages: answer
/// quality 30, self-containment 25, structure 20, statistical density 15,
/// uniqueness 10. Passages of 134-167 words score best because that is the
/// span answer engines most often lift whole.
pub fn score_passage(text: &str, heading: Option<&str>) -> PassageScore {
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len();

    // --- 1. Answer block quality (30) ---
    let mut abq = 0i64;
    if re_definition().is_match(text) {
        abq += 15;
    }
    let first_60: String = words.iter().take(60).copied().collect::<Vec<_>>().join(" ");
    if re_early_answer().is_match(&first_60) {
        abq += 15;
    }
    if heading.is_some_and(|h| h.trim_end().ends_with('?')) {
        abq += 10;
    }
    let sentences: Vec<&str> = text
        .split(['.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
        .collect();
    if !sentences.is_empty() {
        let clear = sentences
            .iter()
            .filter(|s| {
                let n = s.split_whitespace().count();
                (5..=25).contains(&n)
            })
            .count();
        abq += ((clear as f64 / sentences.len() as f64) * 10.0) as i64;
    }
    if re_research().is_match(text) {
        abq += 10;
    }
    let answer_block_quality = abq.min(30);

    // --- 2. Self-containment (25) ---
    let mut sc = 0i64;
    sc += match word_count {
        134..=167 => 10,
        100..=200 => 7,
        80..=250 => 4,
        n if !(30..=400).contains(&n) => 0,
        _ => 2,
    };
    if word_count > 0 {
        let pronouns = re_pronouns().find_iter(text).count();
        let ratio = pronouns as f64 / word_count as f64;
        sc += if ratio < 0.02 {
            8
        } else if ratio < 0.04 {
            5
        } else if ratio < 0.06 {
            3
        } else {
            0
        };
    }
    let proper_nouns = re_proper_nouns().find_iter(text).count();
    sc += if proper_nouns >= 3 {
        7
    } else if proper_nouns >= 1 {
        4
    } else {
        0
    };
    let self_containment = sc.min(25);

    // --- 3. Structural readability (20) ---
    let mut sr = 0i64;
    if !sentences.is_empty() {
        let avg = word_count as f64 / sentences.len() as f64;
        sr += if (10.0..=20.0).contains(&avg) {
            8
        } else if (8.0..=25.0).contains(&avg) {
            5
        } else {
            2
        };
    }
    if re_sequence().is_match(text) {
        sr += 4;
    }
    if re_enumerated().is_match(text) {
        sr += 4;
    }
    if text.contains('\n') {
        sr += 4;
    }
    let structural_readability = sr.min(20);

    // --- 4. Statistical density (15) ---
    let mut sd = 0i64;
    sd += (re_percent().find_iter(text).count() as i64 * 3).min(6);
    sd += (re_dollar().find_iter(text).count() as i64 * 3).min(5);
    sd += (re_counted_nouns().find_iter(text).count() as i64 * 2).min(4);
    if re_recent_year().is_match(text) {
        sd += 2;
    }
    for r in [re_source_attr(), re_source_org(), re_source_paren()] {
        if r.is_match(text) {
            sd += 2;
        }
    }
    let statistical_density = sd.min(15);

    // --- 5. Uniqueness signals (10) ---
    let mut us = 0i64;
    if re_original_data().is_match(text) {
        us += 5;
    }
    if re_case_study().is_match(text) {
        us += 3;
    }
    if re_tool_mention().is_match(text) {
        us += 2;
    }
    let uniqueness_signals = us.min(10);

    let total = answer_block_quality
        + self_containment
        + structural_readability
        + statistical_density
        + uniqueness_signals;

    let (grade, label) = match total {
        80..=i64::MAX => ("A", "Highly Citable"),
        65..=79 => ("B", "Good Citability"),
        50..=64 => ("C", "Moderate Citability"),
        35..=49 => ("D", "Low Citability"),
        _ => ("F", "Poor Citability"),
    };

    let preview_words: Vec<&str> = words.iter().take(30).copied().collect();
    let preview = format!(
        "{}{}",
        preview_words.join(" "),
        if word_count > 30 { "..." } else { "" }
    );

    PassageScore {
        heading: heading.map(|h| h.to_string()),
        word_count,
        total_score: total,
        grade,
        label,
        breakdown: Breakdown {
            answer_block_quality,
            self_containment,
            structural_readability,
            statistical_density,
            uniqueness_signals,
        },
        preview,
    }
}

pub fn citability(url: Option<&str>, file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (source, label) = match (file, url) {
        (Some(path), _) => (std::fs::read_to_string(path)?, path.to_string()),
        (None, Some(u)) => {
            let rec = fetch_record(u, 30, true, None);
            match rec.error {
                Some(e) => return err(format!("Failed to fetch page: {e}")),
                None => (rec.content.unwrap_or_default(), rec.url),
            }
        }
        (None, None) => return err("pass a URL or --file"),
    };

    let blocks = html::content_blocks(&source, 20);
    let scored: Vec<PassageScore> = blocks
        .iter()
        .map(|b| score_passage(&b.content, Some(&b.heading)))
        .collect();

    let (avg, top, bottom, optimal) = if scored.is_empty() {
        (0.0, Vec::new(), Vec::new(), 0)
    } else {
        let avg = scored.iter().map(|s| s.total_score).sum::<i64>() as f64 / scored.len() as f64;
        let mut sorted = scored.clone();
        sorted.sort_by_key(|s| std::cmp::Reverse(s.total_score));
        let top: Vec<PassageScore> = sorted.iter().take(5).cloned().collect();
        let bottom: Vec<PassageScore> = sorted.iter().rev().take(5).cloned().collect();
        let optimal = scored
            .iter()
            .filter(|s| (134..=167).contains(&s.word_count))
            .count();
        (avg, top, bottom, optimal)
    };

    let mut grade_dist = serde_json::Map::new();
    for g in ["A", "B", "C", "D", "F"] {
        grade_dist.insert(
            g.to_string(),
            json!(scored.iter().filter(|s| s.grade == g).count()),
        );
    }

    let result = json!({
        "url": label,
        "total_blocks_analyzed": scored.len(),
        "average_citability_score": (avg * 10.0).round() / 10.0,
        "optimal_length_passages": optimal,
        "grade_distribution": grade_dist,
        "top_5_citable": top,
        "bottom_5_citable": bottom,
        "all_blocks": scored,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {label}");
        println!("Blocks analysed:   {}", scored.len());
        println!("Average score:     {:.1}/100", avg);
        println!("Optimal-length:    {optimal}");
        println!(
            "Grades:            {}",
            serde_json::to_string(&result["grade_distribution"])?
        );
        println!("\nTop passages:");
        for s in &top {
            println!(
                "  [{} {:>3}] {} ({} words)",
                s.grade,
                s.total_score,
                s.heading.as_deref().unwrap_or("(untitled)"),
                s.word_count
            );
        }
    }
    OK
}

// -------------------------------------------------------------- brand scan

/// Platforms ranked by their observed correlation with AI citation.
/// YouTube leads at ~0.737; domain rating trails at ~0.266, which is why
/// this scanner exists alongside the backlink tooling rather than inside it.
const PLATFORM_WEIGHTS: &[(&str, &str, &str)] = &[
    ("youtube", "YouTube", "25%"),
    ("reddit", "Reddit", "25%"),
    ("wikipedia", "Wikipedia", "20%"),
    ("linkedin", "LinkedIn", "15%"),
    ("other", "Other Platforms", "15%"),
];

fn enc(s: &str) -> String {
    http::enc(s)
}

/// Wikipedia and Wikidata expose open APIs, so entity presence is measured
/// rather than guessed. The remaining platforms are gated or JS-rendered;
/// for those we emit the exact search URL and the checklist the agent should
/// run through with its own browsing tools.
fn check_wikipedia(brand: &str) -> serde_json::Value {
    let mut has_wikipedia = false;
    let mut search_results = 0usize;
    let mut has_wikidata = false;
    let mut wikidata_id = serde_json::Value::Null;
    let mut wikidata_description = serde_json::Value::Null;
    // A failed lookup is not the same as "no entry". Report the failure
    // instead of letting it read as a confirmed absence.
    let mut errors: Vec<String> = Vec::new();

    let opts = RequestOptions::with_timeout(15).ua(crate::http::API_USER_AGENT);
    let api = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json",
        enc(brand)
    );
    match http::get(&api, &opts) {
        Err(e) => errors.push(format!("wikipedia: {e}")),
        Ok(resp) if !(200..300).contains(&resp.status) => {
            errors.push(format!("wikipedia: HTTP {}", resp.status))
        }
        Ok(resp) => match serde_json::from_slice::<serde_json::Value>(&resp.body) {
            Err(e) => errors.push(format!("wikipedia: invalid JSON ({e})")),
            Ok(v) => {
                if let Some(results) = v["query"]["search"].as_array() {
                    search_results = results.len();
                    if let Some(first) = results.first() {
                        let title = first["title"].as_str().unwrap_or_default().to_lowercase();
                        if title.contains(&brand.to_lowercase()) {
                            has_wikipedia = true;
                        }
                    }
                }
            }
        },
    }

    let wd = format!(
        "https://www.wikidata.org/w/api.php?action=wbsearchentities&search={}&language=en&format=json",
        enc(brand)
    );
    match http::get(&wd, &opts) {
        Err(e) => errors.push(format!("wikidata: {e}")),
        Ok(resp) if !(200..300).contains(&resp.status) => {
            errors.push(format!("wikidata: HTTP {}", resp.status))
        }
        Ok(resp) => match serde_json::from_slice::<serde_json::Value>(&resp.body) {
            Err(e) => errors.push(format!("wikidata: invalid JSON ({e})")),
            Ok(v) => {
                if let Some(entities) = v["search"].as_array() {
                    if let Some(first) = entities.first() {
                        has_wikidata = true;
                        wikidata_id = first["id"].clone();
                        wikidata_description = first["description"].clone();
                    }
                }
            }
        },
    }

    json!({
        "platform": "Wikipedia",
        "correlation": "High",
        "weight": "20%",
        "has_wikipedia_page": has_wikipedia,
        "wikipedia_search_results": search_results,
        "has_wikidata_entry": has_wikidata,
        "wikidata_id": wikidata_id,
        "wikidata_description": wikidata_description,
        "lookup_errors": errors,
        "lookup_ok": errors.is_empty(),
        "search_url": format!("https://en.wikipedia.org/wiki/Special:Search?search={}", enc(brand)),
        "wikidata_url": format!("https://www.wikidata.org/w/index.php?search={}", enc(brand)),
        "recommendations": [
            "If eligible, create a Wikipedia article (requires notability criteria)",
            "Ensure a Wikidata entry exists with complete structured data",
            "Add sameAs links in schema markup pointing to Wikipedia/Wikidata",
            "Get cited in existing Wikipedia articles as a source",
            "Build notability through press coverage and independent reviews",
        ],
    })
}

pub fn brand_scan(brand: &str, domain: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let youtube = json!({
        "platform": "YouTube",
        "correlation": 0.737,
        "weight": "25%",
        "search_url": format!("https://www.youtube.com/results?search_query={}", enc(brand)),
        "check_instructions": [
            format!("Search YouTube for '{brand}' and check:"),
            "1. Does the brand have an official YouTube channel?",
            "2. Are there videos FROM the brand (tutorials, demos, thought leadership)?",
            "3. Are there videos ABOUT the brand from other creators?",
            "4. What is the view count on brand-related videos?",
            "5. Are there positive reviews or demonstrations?",
        ],
        "recommendations": [
            "Create a YouTube channel if none exists",
            "Publish educational/tutorial content related to your niche",
            "Encourage customers to create review/demo videos",
            "Optimize video titles and descriptions with the brand name",
            "Add timestamps and chapters to improve AI parseability",
            "Include transcripts (auto-generated ones need review)",
        ],
    });

    let reddit = json!({
        "platform": "Reddit",
        "correlation": "High",
        "weight": "25%",
        "search_url": format!("https://www.reddit.com/search/?q={}", enc(brand)),
        "check_instructions": [
            format!("Search Reddit for '{brand}' and check:"),
            "1. Does the brand have its own subreddit?",
            "2. Is the brand discussed in relevant industry subreddits?",
            "3. What is the sentiment (positive, negative, neutral)?",
            "4. Are there recommendation threads mentioning the brand?",
            "5. Does the brand have an official Reddit presence?",
            "6. Are mentions recent (within the last 6 months)?",
        ],
        "recommendations": [
            "Monitor relevant subreddits for brand mentions",
            "Participate authentically in industry discussions (no spam)",
            "Create an official account for customer support",
            "Share valuable content, not just self-promotion",
            "Answer questions about your product category",
            "Reddit rewards authenticity — drop the marketing voice",
        ],
    });

    let linkedin = json!({
        "platform": "LinkedIn",
        "correlation": "Moderate",
        "weight": "15%",
        "search_url": format!("https://www.linkedin.com/search/results/companies/?keywords={}", enc(brand)),
        "check_instructions": [
            format!("Search LinkedIn for '{brand}' and check:"),
            "1. Does the company have a LinkedIn page?",
            "2. How many followers?",
            "3. Is the page active with recent posts?",
            "4. Do employees post thought-leadership content?",
            "5. Are there LinkedIn articles about the brand?",
            "6. Is there engagement on posts?",
        ],
        "recommendations": [
            "Create or optimize the LinkedIn company page",
            "Post regular thought-leadership content",
            "Encourage employees to share company content",
            "Publish long-form LinkedIn articles",
            "Engage with industry discussions and comments",
            "Add the LinkedIn URL to schema sameAs",
        ],
    });

    let other_platforms = [
        (
            "Quora",
            format!("https://www.quora.com/search?q={}", enc(brand)),
        ),
        (
            "Stack Overflow",
            format!("https://stackoverflow.com/search?q={}", enc(brand)),
        ),
        (
            "GitHub",
            format!("https://github.com/search?q={}", enc(brand)),
        ),
        (
            "Crunchbase",
            format!("https://www.crunchbase.com/textsearch?q={}", enc(brand)),
        ),
        (
            "Product Hunt",
            format!("https://www.producthunt.com/search?q={}", enc(brand)),
        ),
        (
            "G2",
            format!("https://www.g2.com/search?query={}", enc(brand)),
        ),
        (
            "Trustpilot",
            format!("https://www.trustpilot.com/search?query={}", enc(brand)),
        ),
    ];
    let mut checked = serde_json::Map::new();
    for (name, url) in &other_platforms {
        checked.insert(
            name.to_string(),
            json!({
                "search_url": url,
                "check_instruction": format!("Search for '{brand}' on {name}"),
            }),
        );
    }
    let other = json!({
        "platform": "Other Platforms",
        "weight": "15%",
        "platforms_checked": checked,
        "recommendations": [
            "Maintain profiles on industry-relevant platforms",
            "Respond to questions on Quora and Stack Overflow",
            "Encourage customer reviews on G2 and Trustpilot",
            "Keep the Crunchbase profile updated (important for B2B)",
            "Open-source contributions on GitHub boost developer brand authority",
            "A Product Hunt launch can generate significant initial reach",
        ],
    });

    let report = json!({
        "brand_name": brand,
        "domain": domain,
        "analysis_date": crate::output::today_utc(),
        "key_insight": "Brand mentions correlate roughly 3x more strongly with AI visibility than backlinks.",
        "platform_weights": PLATFORM_WEIGHTS.iter()
            .map(|(k, n, w)| json!({"key": k, "name": n, "weight": w}))
            .collect::<Vec<_>>(),
        "platforms": {
            "youtube": youtube,
            "reddit": reddit,
            "wikipedia": check_wikipedia(brand),
            "linkedin": linkedin,
            "other": other,
        },
        "overall_recommendations": [
            "Priority 1: YouTube — highest observed correlation with AI citations. Create educational content.",
            "Priority 2: Reddit — build an authentic presence in industry subreddits.",
            "Priority 3: Wikipedia — establish notability through press coverage, then create or improve the entry.",
            "Priority 4: LinkedIn — thought leadership from founders and employees.",
            "Priority 5: Review platforms — G2, Trustpilot, Capterra for social proof.",
            "Cross-platform: keep NAP (name, address, phone) consistent everywhere.",
            "Schema markup: add sameAs linking to every platform profile.",
            "Monitor: set up brand mention alerts across all platforms.",
        ],
    });

    if json {
        print_json(&report)?;
    } else {
        println!("Brand: {brand}");
        if let Some(d) = domain {
            println!("Domain: {d}");
        }
        let wiki = &report["platforms"]["wikipedia"];
        if wiki["lookup_ok"] == true {
            println!(
                "Wikipedia page: {}   Wikidata entry: {}",
                wiki["has_wikipedia_page"], wiki["has_wikidata_entry"]
            );
        } else {
            println!("Wikipedia/Wikidata lookup FAILED — treat the result as unknown, not absent:");
            for e in wiki["lookup_errors"].as_array().unwrap_or(&vec![]) {
                println!("  {}", e.as_str().unwrap_or(""));
            }
        }
        println!("\nPriorities:");
        for r in report["overall_recommendations"].as_array().unwrap() {
            println!("  - {}", r.as_str().unwrap_or_default());
        }
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimal_length_fact_rich_passage_scores_well() {
        let body = "Generative engine optimization is a discipline that improves how \
            answer engines quote a page. According to research from Stanford, 62% of \
            AI citations come from passages between 134 and 167 words. Our analysis of \
            2,400 pages found that self-contained paragraphs earn 3x more citations. \
            First, keep one claim per paragraph. Second, name the entity instead of \
            using a pronoun. Third, attach a number to every claim you make so the \
            passage survives extraction. For example, a pricing page that states \
            $49 per month outperforms one that says affordable pricing in 2026.";
        let s = score_passage(body, Some("What is GEO?"));
        assert!(s.total_score >= 65, "expected B or better, got {s:?}");
        assert_eq!(s.grade, if s.total_score >= 80 { "A" } else { "B" });
    }

    #[test]
    fn vague_pronoun_heavy_passage_scores_poorly() {
        let body = "It is what they said about them and this is that. They think it \
            is this and those are these. It does that.";
        let s = score_passage(body, None);
        assert!(s.total_score < 50, "expected D/F, got {}", s.total_score);
    }
}

//! Content-quality commands: QRG-aligned scoring, AI-pattern rewriting, and
//! claim/citation-gap detection.
//!
//! The AI-pattern catalogue draws on the Wikipedia "AI Cleanup" project's
//! list of LLM-typical phrasings (CC BY-SA 4.0). The selection is
//! deliberately conservative: only phrases that appear disproportionately in
//! generated text and rarely in human writing are included, because the
//! output is advisory — no heuristic can determine authorship.

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

use crate::output::{print_json, read_source, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

/// Padding phrases the Quality Rater Guidelines flag as little-to-no value.
const FILLER_PHRASES: &[&str] = &[
    "it's important to note that",
    "it is important to note that",
    "in this article, we'll explore",
    "in this article we will explore",
    "in today's fast-paced world",
    "in today's digital age",
    "in today's competitive landscape",
    "needless to say",
    "at the end of the day",
    "when it comes to",
    "when all is said and done",
    "in the realm of",
    "in the world of",
    "the bottom line is",
    "without further ado",
    "first and foremost",
    "last but not least",
    "for what it's worth",
    "it goes without saying",
    "as we all know",
    "the truth is that",
    "the fact of the matter is",
    "more often than not",
    "let's dive in",
    "let's dive into",
    "let's take a closer look",
    "let's take a deeper look",
];

const AI_PATTERNS: &[&str] = &[
    "delve into",
    "delve deeper into",
    "in the ever-evolving",
    "ever-evolving landscape",
    "ever-changing landscape",
    "in the dynamic landscape",
    "navigating the",
    "navigate the complexities",
    "tapestry of",
    "rich tapestry",
    "intricate tapestry",
    "embark on a journey",
    "embarking on this",
    "a testament to",
    "a beacon of",
    "the cornerstone of",
    "a cornerstone of",
    "at the heart of",
    "at its core",
    "in essence,",
    "in conclusion,",
    "ultimately,",
    "moreover,",
    "furthermore,",
    "however, it's worth noting",
    "it's worth noting that",
    "it is worth noting that",
    "by leveraging",
    "leverage the power of",
    "leveraging the power of",
    "harness the power of",
    "unlock the potential",
    "unlock the full potential",
    "the realm of possibilities",
    "open up a world of",
    "a world of possibilities",
    "elevate your",
    "transform your",
    "revolutionize the way",
    "game-changer",
    "game-changing",
    "cutting-edge",
    "state-of-the-art",
    "in summary,",
    "to summarize,",
    "to put it simply,",
    "in a nutshell,",
];

fn token_re() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z'\-]*").unwrap())
}
fn number_re() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| Regex::new(r"\b\d+(?:[.,]\d+)?(?:%|st|nd|rd|th)?\b").unwrap())
}
fn entity_re() -> &'static Regex {
    static C: OnceLock<Regex> = OnceLock::new();
    C.get_or_init(|| Regex::new(r"\b(?:[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").unwrap())
}

#[derive(Serialize)]
pub struct QualityReport {
    pub filler_score: i64,
    pub ai_pattern_score: i64,
    pub information_density: f64,
    pub repetition_score: i64,
    pub overall_quality: i64,
    pub flags: Vec<String>,
    pub matches: QualityMatches,
    pub tokens: usize,
    pub unique_tokens: usize,
}

#[derive(Serialize)]
pub struct QualityMatches {
    pub filler: Vec<String>,
    pub ai_patterns: Vec<String>,
}

/// Fraction of bigrams that occur more than once.
fn repetition(tokens: &[String]) -> f64 {
    if tokens.len() < 4 {
        return 0.0;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for w in tokens.windows(2) {
        *counts.entry(format!("{} {}", w[0], w[1])).or_insert(0) += 1;
    }
    let repeated = counts.values().filter(|v| **v > 1).count();
    repeated as f64 / counts.len().max(1) as f64
}

pub fn analyse(text: &str) -> QualityReport {
    if text.trim().is_empty() {
        return QualityReport {
            filler_score: 0,
            ai_pattern_score: 0,
            information_density: 0.0,
            repetition_score: 0,
            overall_quality: 0,
            flags: vec!["empty-input".into()],
            matches: QualityMatches {
                filler: vec![],
                ai_patterns: vec![],
            },
            tokens: 0,
            unique_tokens: 0,
        };
    }

    let tokens: Vec<String> = token_re()
        .find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .collect();
    let n_tokens = tokens.len();
    let unique = {
        let mut set: Vec<&String> = tokens.iter().collect();
        set.sort();
        set.dedup();
        set.len()
    };

    let lowered = text.to_lowercase();
    let filler_hits: Vec<String> = FILLER_PHRASES
        .iter()
        .filter(|p| lowered.contains(**p))
        .map(|p| p.to_string())
        .collect();
    let ai_hits: Vec<String> = AI_PATTERNS
        .iter()
        .filter(|p| lowered.contains(**p))
        .map(|p| p.to_string())
        .collect();

    let entities = entity_re().find_iter(text).count();
    let numbers = number_re().find_iter(text).count();
    let density_per_100 = (entities + numbers) as f64 * 100.0 / n_tokens.max(1) as f64;
    let information_density = (density_per_100 / 10.0).min(1.0);

    let rep = repetition(&tokens);
    let rep_score = (rep * 100.0).round() as i64;

    // Scale to per-1000 tokens so scores are comparable across page lengths.
    let scale = (n_tokens as f64 / 1000.0).max(1.0);
    let filler_score = ((filler_hits.len() as f64 / scale) * 25.0).round().min(100.0) as i64;
    let ai_pattern_score = ((ai_hits.len() as f64 / scale) * 15.0).round().min(100.0) as i64;

    let mut flags = Vec::new();
    if filler_score >= 50 {
        flags.push("filler".to_string());
    }
    if ai_pattern_score >= 40 {
        flags.push("ai-patterns".to_string());
    }
    if information_density < 0.20 {
        flags.push("low-density".to_string());
    }
    if rep_score >= 30 {
        flags.push("repetitive".to_string());
    }
    if n_tokens < 300 {
        flags.push("thin-content".to_string());
    }

    let overall = (100 - filler_score) as f64 * 0.25
        + (100 - ai_pattern_score) as f64 * 0.25
        + information_density * 100.0 * 0.25
        + (100 - rep_score) as f64 * 0.15
        + (n_tokens as f64 / 10.0).min(100.0) * 0.10;

    QualityReport {
        filler_score,
        ai_pattern_score,
        information_density: (information_density * 1000.0).round() / 1000.0,
        repetition_score: rep_score,
        overall_quality: overall.round() as i64,
        flags,
        matches: QualityMatches {
            filler: filler_hits,
            ai_patterns: ai_hits,
        },
        tokens: n_tokens,
        unique_tokens: unique,
    }
}

pub fn quality(source: &str, threshold: i64, json: bool) -> CmdResult<ExitCode> {
    let raw = read_source(source)?;
    // Accept HTML transparently: an audit usually pipes a fetched page here.
    let text = if raw.trim_start().starts_with('<') {
        crate::html::visible_text(&raw)
    } else {
        raw
    };
    let result = analyse(&text);

    if json {
        print_json(&result)?;
    } else {
        println!("Overall quality:       {}/100", result.overall_quality);
        println!("  Filler score:        {}/100 (higher = worse)", result.filler_score);
        println!("  AI-pattern score:    {}/100 (higher = worse)", result.ai_pattern_score);
        println!("  Information density: {:.2}", result.information_density);
        println!("  Repetition:          {}/100 (higher = worse)", result.repetition_score);
        println!("  Tokens:              {} ({} unique)", result.tokens, result.unique_tokens);
        if !result.flags.is_empty() {
            println!("  Flags:               {}", result.flags.join(", "));
        }
        if !result.matches.filler.is_empty() {
            println!("  Filler hits:         {}", result.matches.filler.join(", "));
        }
        if !result.matches.ai_patterns.is_empty() {
            println!("  AI-pattern hits:     {}", result.matches.ai_patterns.join(", "));
        }
    }

    Ok(if result.overall_quality >= threshold {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// ---------------------------------------------------------------- humanize

/// `(pattern, replacement, label)`. Every swap is deterministic and 1:1;
/// unknown idiom is left alone rather than paraphrased.
const REPLACEMENTS: &[(&str, &str, &str)] = &[
    (r"(?i)\bdelve\s+deeper\s+into\b", "explore", "delve-deeper-into"),
    (r"(?i)\bdelve\s+into\b", "explore", "delve-into"),
    (r"(?i)\bin\s+the\s+ever-evolving\s+landscape\s+of\b", "in", "ever-evolving-landscape"),
    (r"(?i)\bin\s+the\s+ever-evolving\s+world\s+of\b", "in", "ever-evolving-world"),
    (r"(?i)\bever-evolving\b", "changing", "ever-evolving"),
    (r"(?i)\bever-changing\b", "changing", "ever-changing"),
    (r"(?i)\bnavigating\s+the\s+complexities\s+of\b", "handling", "navigating-complexities"),
    (r"(?i)\btapestry\s+of\b", "range of", "tapestry-of"),
    (r"(?i)\b(?:rich|intricate|complex)\s+tapestry\b", "range", "rich-tapestry"),
    (r"(?i)\bembark\s+on\s+a\s+journey\b", "begin", "embark-journey"),
    (r"(?i)\ba\s+testament\s+to\b", "evidence of", "testament-to"),
    (r"(?i)\ba\s+beacon\s+of\b", "a leader in", "beacon-of"),
    (r"(?i)\b(?:the\s+|a\s+)?cornerstone\s+of\b", "central to", "cornerstone-of"),
    (r"(?i)\bat\s+the\s+heart\s+of\b", "central to", "at-the-heart-of"),
    (r"(?i)\bin\s+essence,\s*", "", "in-essence"),
    (r"(?i)\bin\s+conclusion,\s*", "", "in-conclusion"),
    (r"(?i)\bultimately,\s*", "", "ultimately-comma"),
    (r"(?i)\bmoreover,\s*", "", "moreover-comma"),
    (r"(?i)\bfurthermore,\s*", "", "furthermore-comma"),
    (r"(?i)\bhowever,\s+it(?:'?s|\s+is)\s+worth\s+noting\s+that\b", "however,", "worth-noting-clause"),
    (r"(?i)\bit(?:'?s|\s+is)\s+worth\s+noting\s+that\b", "note:", "worth-noting"),
    (r"(?i)\bby\s+leveraging\b", "by using", "by-leveraging"),
    (r"(?i)\bleverage\s+the\s+power\s+of\b", "use", "leverage-power"),
    (r"(?i)\bleveraging\s+the\s+power\s+of\b", "using", "leveraging-power"),
    (r"(?i)\bharness\s+the\s+power\s+of\b", "use", "harness-power"),
    (r"(?i)\bunlock\s+(?:the\s+(?:full\s+)?)?potential\b", "use", "unlock-potential"),
    (r"(?i)\bopen\s+up\s+a\s+world\s+of\b", "enable", "open-world"),
    (r"(?i)\ba\s+world\s+of\s+possibilities\b", "options", "world-possibilities"),
    (r"(?i)\belevate\s+your\b", "improve your", "elevate-your"),
    (r"(?i)\btransform\s+your\b", "improve your", "transform-your"),
    (r"(?i)\brevolutionize\s+the\s+way\b", "change how", "revolutionize-the-way"),
    (r"(?i)\bgame-?changer\b", "major advance", "game-changer"),
    (r"(?i)\bcutting-?edge\b", "modern", "cutting-edge"),
    (r"(?i)\bstate-of-the-art\b", "modern", "state-of-the-art"),
    (r"(?i)\bin\s+summary,\s*", "", "in-summary"),
    (r"(?i)\bto\s+summarize,\s*", "", "to-summarize"),
    (r"(?i)\bto\s+put\s+it\s+simply,\s*", "", "to-put-simply"),
    (r"(?i)\bin\s+a\s+nutshell,\s*", "", "in-nutshell"),
    (r"(?i)\bit(?:'?s|\s+is)\s+important\s+to\s+note\s+that\b", "note:", "important-note"),
    (
        r"(?i)\bin\s+today'?s\s+(?:fast-paced|digital|competitive)\s+(?:world|age|landscape)\b",
        "today",
        "today-cliche",
    ),
    (r"(?i)\bneedless\s+to\s+say,?\s*", "", "needless-to-say"),
    (r"(?i)\bat\s+the\s+end\s+of\s+the\s+day\b", "ultimately", "end-of-the-day"),
    (r"(?i)\bwhen\s+it\s+comes\s+to\b", "for", "when-it-comes-to"),
    (r"(?i)\bfirst\s+and\s+foremost,?\s*", "first,", "first-and-foremost"),
    (r"(?i)\blast\s+but\s+not\s+least,?\s*", "finally,", "last-but-not-least"),
    (r"(?i)\blet'?s\s+dive\s+(?:in|into)\b", "starting with", "let-us-dive"),
    (r"(?i)\blet'?s\s+take\s+a\s+(?:closer|deeper)\s+look\b", "look at", "let-us-take-look"),
];

#[derive(Serialize)]
pub struct Change {
    pub label: String,
    pub from: String,
    pub to: String,
}

#[derive(Serialize)]
pub struct HumanizeResult {
    pub cleaned: String,
    pub changes: Vec<Change>,
    pub change_count: usize,
}

/// Keep the original leading case so "Leverage X" becomes "Use X".
fn preserve_case(original: &str, replacement: &str) -> String {
    if replacement.is_empty() {
        return String::new();
    }
    let first_upper = original.chars().next().is_some_and(|c| c.is_uppercase());
    let repl_upper = replacement.chars().next().is_some_and(|c| c.is_uppercase());
    if first_upper && !repl_upper {
        let mut chars = replacement.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    } else {
        replacement.to_string()
    }
}

pub fn humanize_text(text: &str) -> HumanizeResult {
    let mut cleaned = text.to_string();
    let mut changes = Vec::new();

    for (pattern, replacement, label) in REPLACEMENTS {
        let re = Regex::new(pattern).expect("static regex");
        let mut out = String::with_capacity(cleaned.len());
        let mut last = 0;
        for m in re.find_iter(&cleaned) {
            out.push_str(&cleaned[last..m.start()]);
            let new = preserve_case(m.as_str(), replacement);
            changes.push(Change {
                label: label.to_string(),
                from: m.as_str().to_string(),
                to: new.clone(),
            });
            out.push_str(&new);
            last = m.end();
        }
        out.push_str(&cleaned[last..]);
        cleaned = out;
    }

    // Tidy the whitespace deletions leave behind, without touching newlines.
    let collapse = Regex::new(r"[ \t]{2,}").unwrap();
    cleaned = collapse.replace_all(&cleaned, " ").into_owned();
    let before_punct = Regex::new(r" ([,.;:!?])").unwrap();
    cleaned = before_punct.replace_all(&cleaned, "$1").into_owned();

    HumanizeResult {
        change_count: changes.len(),
        cleaned,
        changes,
    }
}

pub fn humanize(source: &str, output: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let text = read_source(source)?;
    let result = humanize_text(&text);

    if json {
        print_json(&result)?;
        return OK;
    }

    match output {
        Some(path) => {
            std::fs::write(path, &result.cleaned)?;
            eprintln!("Wrote {path} ({} replacements)", result.change_count);
        }
        None => print!("{}", result.cleaned),
    }

    if result.change_count > 0 {
        eprintln!("\n--- {} replacements ---", result.change_count);
        let mut seen: Vec<&str> = Vec::new();
        for c in &result.changes {
            if seen.contains(&c.label.as_str()) {
                continue;
            }
            seen.push(&c.label);
            eprintln!("  {:?} -> {:?}", c.from, c.to);
        }
    }
    OK
}

// ------------------------------------------------------------------ verify

/// Claim patterns, most specific first — the first match wins for a given
/// span so "47% of marketers" is not double-counted as statistic + quantity.
const CLAIM_PATTERNS: &[(&str, &str)] = &[
    (r"(?i)\b\d+(?:\.\d+)?\s*%\s+of\s+[a-zA-Z]+(?:\s+[a-zA-Z]+){0,4}", "statistic"),
    (r"\b\d+(?:\.\d+)?\s*%", "statistic"),
    (r"(?i)\$\s?\d+(?:\.\d+)?\s*(?:million|billion|trillion|k|M|B)\b", "quantity"),
    (r"(?i)\b\d{1,3}(?:,\d{3})+(?:\.\d+)?\s+\w+", "quantity"),
    (r"(?i)\b\d+(?:\.\d+)?\s*(?:million|billion|trillion)\b", "quantity"),
    (
        r"\baccording\s+to\s+(?:a\s+)?(?:[A-Z][a-z]+\s+){1,4}(?:study|report|survey|analysis|paper)\b",
        "authority",
    ),
    (
        r"(?i)\b(?:Forrester|Gartner|McKinsey|Pew|Nielsen|Statista|Deloitte|Edelman|MIT|Stanford|Harvard|Wharton)\s+(?:said|reports?|found|noted)",
        "authority",
    ),
    (r"\bin\s+(?:19|20)\d{2}\b", "temporal"),
    (r"\bby\s+20\d{2}\b", "temporal"),
    (
        r"(?i)\b\d+(?:\.\d+)?\s*(?:x|times)\s+(?:more|less|faster|slower|higher|lower|better|worse)\b",
        "comparative",
    ),
    (r"(?i)\b(?:twice|thrice|half)\s+as\s+\w+", "comparative"),
];

/// Bare "see" and "per" are excluded: they are common English words that
/// produce far too many false positives. We require an explicit link,
/// footnote, or attribution form.
const CITATION_PATTERNS: &[&str] = &[
    r"\[[^\]]+\]\(https?://[^)]+\)",
    r#"(?i)<a\s+[^>]*href=["']https?://[^"']+["']"#,
    r"\[\^?\d+\]",
    r#"(?i)@type\s*:\s*["']Citation["']"#,
    r"(?i)\b(?:source\s*:|via\s*:|see\s+also\s*:|cited\s+(?:in|by)|according\s+to|per)\s+[A-Z]",
];

#[derive(Serialize, Debug)]
pub struct Claim {
    pub text: String,
    pub kind: String,
    pub position: usize,
    pub has_citation: bool,
    pub nearby_citation: Option<String>,
}

#[derive(Serialize)]
pub struct VerifyResult {
    pub claims: Vec<Claim>,
    pub claim_count: usize,
    pub uncited_count: usize,
    pub uncited_ratio: f64,
}

fn citation_near(text: &str, position: usize, window: usize) -> Option<String> {
    let start = position.saturating_sub(window);
    let end = (position + window).min(text.len());
    // Snap to char boundaries so slicing never panics on multi-byte input.
    let start = (start..=position).find(|i| text.is_char_boundary(*i))?;
    let end = (position..=end).rev().find(|i| text.is_char_boundary(*i))?;
    let snippet = &text[start..end];
    for pattern in CITATION_PATTERNS {
        let re = Regex::new(pattern).expect("static regex");
        if let Some(m) = re.find(snippet) {
            return Some(crate::output::truncate(m.as_str(), 80));
        }
    }
    None
}

pub fn extract_claims(text: &str) -> Vec<Claim> {
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for (pattern, label) in CLAIM_PATTERNS {
        let re = Regex::new(pattern).expect("static regex");
        for m in re.find_iter(text) {
            let overlaps = spans
                .iter()
                .any(|(s, e, _)| (*s <= m.start() && m.start() < *e) || (*s < m.end() && m.end() <= *e));
            if overlaps {
                continue;
            }
            spans.push((m.start(), m.end(), label));
        }
    }
    spans.sort_by_key(|(s, _, _)| *s);

    spans
        .into_iter()
        .map(|(start, end, label)| {
            let cite = citation_near(text, start, 200);
            Claim {
                text: text[start..end].trim().to_string(),
                kind: label.to_string(),
                position: start,
                has_citation: cite.is_some(),
                nearby_citation: cite,
            }
        })
        .collect()
}

pub fn verify(source: &str, threshold: f64, json: bool) -> CmdResult<ExitCode> {
    let text = read_source(source)?;
    let claims = extract_claims(&text);
    let uncited = claims.iter().filter(|c| !c.has_citation).count();
    let ratio = if claims.is_empty() {
        0.0
    } else {
        (uncited as f64 / claims.len() as f64 * 1000.0).round() / 1000.0
    };
    let result = VerifyResult {
        claim_count: claims.len(),
        uncited_count: uncited,
        uncited_ratio: ratio,
        claims,
    };

    if json {
        print_json(&result)?;
    } else {
        println!("Claims:          {}", result.claim_count);
        println!("Uncited:         {}", result.uncited_count);
        println!("Uncited ratio:   {:.2}", result.uncited_ratio);
        for c in &result.claims {
            let mark = if c.has_citation { "ok " } else { "GAP" };
            println!("  [{mark} {:<11}] {:?}", c.kind, c.text);
        }
    }

    Ok(if result.uncited_ratio <= threshold {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filler_and_ai_patterns_are_flagged() {
        let text = "In today's fast-paced world, let's dive into the ever-evolving \
                    landscape of marketing. It's important to note that this is a \
                    game-changer. In conclusion, we must delve into cutting-edge ideas.";
        let r = analyse(text);
        assert!(!r.matches.filler.is_empty());
        assert!(!r.matches.ai_patterns.is_empty());
        assert!(r.flags.contains(&"thin-content".to_string()));
    }

    #[test]
    fn humanize_preserves_leading_case() {
        let r = humanize_text("Leverage the power of data. delve into the details.");
        assert!(r.cleaned.starts_with("Use data."), "got {:?}", r.cleaned);
        assert!(r.cleaned.contains("explore the details"));
        assert_eq!(r.change_count, 2);
    }

    #[test]
    fn claims_detect_citation_gaps() {
        // The citation window is +/- 200 characters, so the uncited claim has
        // to sit outside it or it borrows the other claim's source.
        let filler = "x".repeat(400);
        let text = format!(
            "Revenue grew 47% last year. {filler} \
             Adoption hit 62% of teams ([Source](https://example.com/report))."
        );
        let claims = extract_claims(&text);
        assert!(claims.len() >= 2, "got {claims:?}");
        assert!(!claims[0].has_citation, "first claim should be uncited");
        assert!(
            claims.last().unwrap().has_citation,
            "linked claim should be cited"
        );
    }

    #[test]
    fn multibyte_text_survives_the_citation_window() {
        // The window is sliced by byte offset; without char-boundary snapping
        // this panics on CJK text.
        let text = format!(
            "{pad} 47% of teams shipped weekly. {pad}",
            pad = "日本語のテキストがここに入ります。".repeat(20)
        );
        let claims = extract_claims(&text);
        assert_eq!(claims.len(), 1, "got {claims:?}");
        assert!(!claims[0].has_citation);
    }
}

//! HTML → SEO element extraction.
//!
//! Port of `parse_html.py` plus the shared helpers the GEO commands need
//! (visible-text extraction, heading-delimited content blocks, security
//! headers, SSR detection).

use std::collections::BTreeMap;

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use url::Url;

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ImageInfo {
    pub src: String,
    pub alt: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
    pub loading: Option<String>,
    pub lazy_method: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct LinkInfo {
    pub href: String,
    pub text: String,
    pub rel: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Links {
    pub internal: Vec<LinkInfo>,
    pub external: Vec<LinkInfo>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Hreflang {
    pub lang: String,
    pub href: Option<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ParsedPage {
    pub title: Option<String>,
    pub meta_description: Option<String>,
    pub meta_robots: Option<String>,
    pub canonical: Option<String>,
    pub h1: Vec<String>,
    pub h2: Vec<String>,
    pub h3: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub h1_suspicious: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub h2_suspicious: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub h3_suspicious: Vec<String>,
    pub images: Vec<ImageInfo>,
    pub links: Links,
    pub schema: Vec<serde_json::Value>,
    pub open_graph: BTreeMap<String, String>,
    pub twitter_card: BTreeMap<String, String>,
    pub word_count: usize,
    pub hreflang: Vec<Hreflang>,
}

const PERFMATTERS_ATTRS: &[&str] = &["data-perfmatters-src", "data-perfmatters-srcset"];
const EWWW_ATTRS: &[&str] = &["data-ewww-src", "data-eio"];
const GENERIC_LAZY_ATTRS: &[&str] = &["data-src", "data-lazy-src", "data-original", "data-srcset"];
const PERFMATTERS_CLASSES: &[&str] = &["perfmatters-lazy", "perfmatters-lazy-loaded"];
const EWWW_CLASSES: &[&str] = &["lazyload-eio", "lazyloaded-eio"];
const GENERIC_LAZY_CLASSES: &[&str] = &["lazyload", "lazyloaded", "lazy", "lazy-loaded"];

/// Classify how an `<img>` defers loading. Specific plugin stacks are
/// checked before the generic bucket so reports can attribute the
/// optimisation to the right plugin.
fn detect_lazy_method(el: &ElementRef) -> String {
    if el
        .value()
        .attr("loading")
        .map(|v| v.eq_ignore_ascii_case("lazy"))
        .unwrap_or(false)
    {
        return "native".into();
    }
    let classes: Vec<String> = el
        .value()
        .attr("class")
        .map(|c| {
            c.split_whitespace()
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let has_attr = |names: &[&str]| {
        names
            .iter()
            .any(|n| el.value().attr(n).is_some_and(|v| !v.is_empty()))
    };
    let has_class = |names: &[&str]| names.iter().any(|n| classes.iter().any(|c| c == n));

    if has_attr(PERFMATTERS_ATTRS) || has_class(PERFMATTERS_CLASSES) {
        return "perfmatters".into();
    }
    if has_attr(EWWW_ATTRS) || has_class(EWWW_CLASSES) {
        return "ewww".into();
    }
    if has_attr(GENERIC_LAZY_ATTRS) || has_class(GENERIC_LAZY_CLASSES) {
        return "js-generic".into();
    }
    "none".into()
}

fn text_of(el: &ElementRef) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn joined_text(el: &ElementRef) -> String {
    let raw: String = el
        .text()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    raw
}

/// Headings that are almost certainly counters or stats rather than topics.
fn is_suspicious_heading(text: &str) -> bool {
    let t = text.trim();
    if t.chars().count() <= 3 {
        return true;
    }
    let stripped: String = t
        .chars()
        .filter(|c| !matches!(c, ',' | '.' | '+' | '-' | '%' | ' '))
        .collect();
    !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit())
}

pub fn parse(html: &str, base_url: Option<&str>) -> ParsedPage {
    let doc = Html::parse_document(html);
    let mut out = ParsedPage::default();

    if let Some(t) = doc.select(&sel("title")).next() {
        out.title = Some(text_of(&t));
    }

    for meta in doc.select(&sel("meta")) {
        let name = meta
            .value()
            .attr("name")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let property = meta
            .value()
            .attr("property")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let content = meta.value().attr("content").unwrap_or_default().to_string();

        if name == "description" {
            out.meta_description = Some(content.clone());
        } else if name == "robots" {
            out.meta_robots = Some(content.clone());
        }
        if property.starts_with("og:") {
            out.open_graph.insert(property.clone(), content.clone());
        }
        if name.starts_with("twitter:") {
            out.twitter_card.insert(name.clone(), content.clone());
        }
    }

    if let Some(c) = doc.select(&sel(r#"link[rel~="canonical"]"#)).next() {
        out.canonical = c.value().attr("href").map(|s| s.to_string());
    }

    for link in doc.select(&sel(r#"link[rel~="alternate"]"#)) {
        if let Some(lang) = link.value().attr("hreflang") {
            out.hreflang.push(Hreflang {
                lang: lang.to_string(),
                href: link.value().attr("href").map(|s| s.to_string()),
            });
        }
    }

    for tag in ["h1", "h2", "h3"] {
        for h in doc.select(&sel(tag)) {
            let text = text_of(&h);
            if text.is_empty() {
                continue;
            }
            let susp = is_suspicious_heading(&text);
            match tag {
                "h1" => {
                    out.h1.push(text.clone());
                    if susp {
                        out.h1_suspicious.push(text);
                    }
                }
                "h2" => {
                    out.h2.push(text.clone());
                    if susp {
                        out.h2_suspicious.push(text);
                    }
                }
                _ => {
                    out.h3.push(text.clone());
                    if susp {
                        out.h3_suspicious.push(text);
                    }
                }
            }
        }
    }

    let base = base_url.and_then(|b| Url::parse(b).ok());

    for img in doc.select(&sel("img")) {
        let mut src = img.value().attr("src").unwrap_or_default().to_string();
        if let (Some(b), false) = (&base, src.is_empty()) {
            if let Ok(joined) = b.join(&src) {
                src = joined.to_string();
            }
        }
        out.images.push(ImageInfo {
            src,
            alt: img.value().attr("alt").map(|s| s.to_string()),
            width: img.value().attr("width").map(|s| s.to_string()),
            height: img.value().attr("height").map(|s| s.to_string()),
            loading: img.value().attr("loading").map(|s| s.to_string()),
            lazy_method: detect_lazy_method(&img),
        });
    }

    if let Some(b) = &base {
        let base_host = b.host_str().unwrap_or_default().to_string();
        for a in doc.select(&sel("a[href]")) {
            let href = a.value().attr("href").unwrap_or_default();
            if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
                continue;
            }
            let full = match b.join(href) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let mut text = joined_text(&a);
            if text.chars().count() > 100 {
                text = text.chars().take(100).collect();
            }
            let info = LinkInfo {
                href: full.to_string(),
                text,
                rel: a
                    .value()
                    .attr("rel")
                    .map(|r| r.split_whitespace().map(|s| s.to_string()).collect())
                    .unwrap_or_default(),
            };
            if full.host_str().unwrap_or_default() == base_host {
                out.links.internal.push(info);
            } else {
                out.links.external.push(info);
            }
        }
    }

    out.schema = extract_jsonld(&doc);
    out.word_count = word_count(&visible_text(html));
    out
}

/// Pull every JSON-LD block, flattening `@graph` containers and top-level
/// arrays so each `@type` is an independent entry.
pub fn extract_jsonld(doc: &Html) -> Vec<serde_json::Value> {
    let mut found = Vec::new();
    for script in doc.select(&sel(r#"script[type="application/ld+json"]"#)) {
        let raw: String = script.text().collect();
        let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match parsed {
            serde_json::Value::Object(ref map) if map.contains_key("@graph") => {
                if let Some(serde_json::Value::Array(items)) = map.get("@graph") {
                    for item in items {
                        if item.is_object() {
                            found.push(item.clone());
                        }
                    }
                } else {
                    found.push(parsed);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    if item.is_object() {
                        found.push(item);
                    }
                }
            }
            other => found.push(other),
        }
    }
    found
}

/// Elements whose text is chrome rather than content. `parse_html.py`
/// decomposes these before counting words; we strip them from the source
/// so the same text survives.
const NON_CONTENT_TAGS: &[&str] = &[
    "script", "style", "nav", "footer", "header", "noscript", "template",
];

/// Remove whole non-content subtrees from an HTML source string.
pub fn strip_non_content(html: &str, tags: &[&str]) -> String {
    let mut out = html.to_string();
    for tag in tags {
        let pattern = format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>");
        if let Ok(re) = regex::Regex::new(&pattern) {
            out = re.replace_all(&out, " ").into_owned();
        }
        // Self-closing / unterminated forms leave the open tag behind.
        let solo = format!(r"(?is)<{tag}\b[^>]*/?>");
        if let Ok(re) = regex::Regex::new(&solo) {
            out = re.replace_all(&out, " ").into_owned();
        }
    }
    out
}

/// Visible text with script/style/nav/footer/header removed — the same
/// stripping `parse_html.py` performs before counting words.
pub fn visible_text(html: &str) -> String {
    let stripped = strip_non_content(html, NON_CONTENT_TAGS);
    let doc = Html::parse_document(&stripped);
    doc.root_element()
        .text()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn word_count(text: &str) -> usize {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .count()
}

#[derive(Debug, Serialize, Clone)]
pub struct ContentBlock {
    pub heading: String,
    pub content: String,
    pub word_count: usize,
}

/// Split a page into heading-delimited content blocks. Non-content
/// containers are dropped first so navigation copy never lands in a block.
pub fn content_blocks(html: &str, min_words: usize) -> Vec<ContentBlock> {
    let stripped = strip_non_content(
        html,
        &[
            "script", "style", "nav", "footer", "header", "aside", "form", "noscript",
        ],
    );
    let doc = Html::parse_document(&stripped);

    let mut blocks = Vec::new();
    let mut current_heading = "Introduction".to_string();
    let mut current: Vec<String> = Vec::new();

    let flush = |heading: &str, parts: &mut Vec<String>, blocks: &mut Vec<ContentBlock>| {
        if parts.is_empty() {
            return;
        }
        let combined = parts.join(" ");
        parts.clear();
        let wc = combined.split_whitespace().count();
        if wc >= min_words {
            blocks.push(ContentBlock {
                heading: heading.to_string(),
                content: combined,
                word_count: wc,
            });
        }
    };

    for el in doc.select(&sel("h1, h2, h3, h4, p, ul, ol, table, blockquote")) {
        let name = el.value().name();
        if name.starts_with('h') && name.len() == 2 {
            flush(&current_heading, &mut current, &mut blocks);
            current_heading = text_of(&el);
        } else {
            let text = joined_text(&el);
            if !text.is_empty() && text.split_whitespace().count() >= 5 {
                current.push(text);
            }
        }
    }
    flush(&current_heading, &mut current, &mut blocks);
    blocks
}

/// Security headers a technical audit reports on.
pub const SECURITY_HEADERS: &[&str] = &[
    "strict-transport-security",
    "content-security-policy",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
    "permissions-policy",
];

/// Server-rendered content check: a framework root div with almost no text
/// on a page that is also thin overall indicates client-only rendering.
/// Prerendered/SSR sites keep substantial text despite having such a root.
pub fn ssr_assessment(html: &str, page_word_count: usize) -> (bool, Vec<String>) {
    let doc = Html::parse_document(html);
    let mut issues = Vec::new();
    let mut has_ssr = true;
    for el in doc.select(&sel("[id]")) {
        let id = el
            .value()
            .attr("id")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !(id.contains("app")
            || id.contains("root")
            || id.contains("__next")
            || id.contains("__nuxt"))
        {
            continue;
        }
        let inner = text_of(&el);
        if inner.chars().count() < 50 && page_word_count < 200 {
            has_ssr = false;
            issues.push(format!(
                "Possible client-side only rendering detected: #{id} has minimal \
                 server-rendered content ({page_word_count} words on page)"
            ));
        }
    }
    (has_ssr, issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<!doctype html><html><head>
      <title>Widget Co</title>
      <meta name="description" content="We sell widgets.">
      <meta name="robots" content="index,follow">
      <meta property="og:title" content="Widget Co">
      <meta name="twitter:card" content="summary">
      <link rel="canonical" href="https://widget.example/">
      <link rel="alternate" hreflang="de" href="https://widget.example/de">
      <script type="application/ld+json">{"@graph":[{"@type":"Organization","name":"Widget"}]}</script>
      </head><body>
      <nav>Nav text here</nav>
      <h1>Widgets for everyone</h1><h2>99</h2>
      <p>Widgets are small mechanical parts used in many industries today.</p>
      <img src="/a.png" alt="a" loading="lazy">
      <img src="/b.png" data-src="/b-real.png">
      <a href="/about">About</a><a href="https://other.example/x">Out</a>
      </body></html>"#;

    #[test]
    fn extracts_core_elements() {
        let p = parse(SAMPLE, Some("https://widget.example/"));
        assert_eq!(p.title.as_deref(), Some("Widget Co"));
        assert_eq!(p.meta_description.as_deref(), Some("We sell widgets."));
        assert_eq!(p.canonical.as_deref(), Some("https://widget.example/"));
        assert_eq!(p.h1, vec!["Widgets for everyone"]);
        assert_eq!(p.h2_suspicious, vec!["99"]);
        assert_eq!(p.hreflang.len(), 1);
        assert_eq!(p.schema.len(), 1);
        assert_eq!(p.open_graph.get("og:title").unwrap(), "Widget Co");
        assert_eq!(p.twitter_card.get("twitter:card").unwrap(), "summary");
        assert_eq!(p.links.internal.len(), 1);
        assert_eq!(p.links.external.len(), 1);
        assert_eq!(p.images[0].lazy_method, "native");
        assert_eq!(p.images[1].lazy_method, "js-generic");
        assert!(p.word_count > 5);
    }

    #[test]
    fn visible_text_drops_nav() {
        let text = visible_text(SAMPLE);
        assert!(!text.contains("Nav text here"));
        assert!(text.contains("Widgets are small"));
    }

    #[test]
    fn blocks_group_under_headings() {
        let html = r#"<html><body>
            <nav>skip me entirely</nav>
            <h2>What is a widget</h2>
            <p>A widget is a small mechanical part used across many industries.</p>
            <p>Widgets are measured in millimetres and rated for load.</p>
            <h2>Pricing</h2>
            <p>Widgets cost between four and nine dollars per unit at retail.</p>
            </body></html>"#;
        let blocks = content_blocks(html, 5);
        let headings: Vec<&str> = blocks.iter().map(|b| b.heading.as_str()).collect();
        assert_eq!(headings, vec!["What is a widget", "Pricing"]);
        // Consecutive paragraphs under one heading join into a single block.
        assert!(blocks[0].content.contains("millimetres"));
        assert!(blocks[0].word_count > blocks[1].word_count);
        assert!(!blocks.iter().any(|b| b.content.contains("skip me")));
    }
}

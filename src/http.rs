//! Shared HTTP client.
//!
//! Every outbound request in the binary goes through here so the SSRF
//! resolver, the timeout policy, and the charset-aware decoder are applied
//! uniformly. Redirects are followed by `ureq` but each hop is re-resolved
//! through [`SafeResolver`], so a redirect to a private address fails the
//! connection rather than the check.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::safety::{validate_url_strict, SafeResolver, UrlSafetyError};

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/150.0.7871.114 Safari/537.36 seogeo/0.1";

/// Wikimedia's API policy requires a descriptive User-Agent that identifies
/// the client and a contact URL; browser-like strings are rate-limited or
/// refused outright.
pub const API_USER_AGENT: &str = concat!(
    "seogeo/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/asale-ai/seo-geo-skill)"
);

/// Googlebot UA, used to detect prerender / dynamic-rendering setups by
/// comparing response size against the default UA.
pub const GOOGLEBOT_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";

/// User agents of the AI crawlers that feed answer engines.
pub const AI_CRAWLERS: &[(&str, &str)] = &[
    ("GPTBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.2; +https://openai.com/gptbot)"),
    ("ClaudeBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +https://www.anthropic.com/claude-bot)"),
    ("PerplexityBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; PerplexityBot/1.0; +https://perplexity.ai/perplexitybot)"),
    ("GoogleBot", GOOGLEBOT_USER_AGENT),
    ("BingBot", "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)"),
];

/// The AI crawler tokens a robots.txt audit reports on.
pub const AI_CRAWLER_TOKENS: &[&str] = &[
    "GPTBot",
    "OAI-SearchBot",
    "ChatGPT-User",
    "ClaudeBot",
    "anthropic-ai",
    "Claude-SearchBot",
    "PerplexityBot",
    "Perplexity-User",
    "CCBot",
    "Bytespider",
    "cohere-ai",
    "Google-Extended",
    "GoogleOther",
    "Applebot-Extended",
    "FacebookBot",
    "Amazonbot",
    "meta-externalagent",
    "MistralAI-User",
];

/// RFC 3986 unreserved characters stay literal; everything else is
/// percent-encoded. `NON_ALPHANUMERIC` also escapes `-._~`, which some APIs
/// reject inside a URL-valued query parameter.
const QUERY_SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode a value for use in a query string.
pub fn enc(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, QUERY_SAFE).to_string()
}

#[derive(Debug)]
pub struct Response {
    pub url: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Response {
    /// Decode the body deterministically: BOM, then `Content-Type` charset,
    /// then `<meta charset>`, then UTF-8 with replacement.
    pub fn text(&self) -> String {
        decode_body(&self.body, self.header("content-type").unwrap_or_default())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
    }
}

pub fn decode_body(raw: &[u8], content_type: &str) -> String {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&raw[3..]).into_owned();
    }
    if raw.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(&raw[2..]);
        return s.into_owned();
    }
    if raw.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(&raw[2..]);
        return s.into_owned();
    }
    if let Some(cs) = charset_from_content_type(content_type) {
        if let Some(enc) = encoding_rs::Encoding::for_label(cs.as_bytes()) {
            let (s, _, _) = enc.decode(raw);
            return s.into_owned();
        }
    }
    if let Some(cs) = charset_from_meta(raw) {
        if let Some(enc) = encoding_rs::Encoding::for_label(cs.as_bytes()) {
            let (s, _, _) = enc.decode(raw);
            return s.into_owned();
        }
    }
    String::from_utf8_lossy(raw).into_owned()
}

fn charset_from_content_type(ct: &str) -> Option<String> {
    let lower = ct.to_ascii_lowercase();
    let idx = lower.find("charset")?;
    let rest = &ct[idx + "charset".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == ';' || c == ',' || c == '"' || c == '\'' || c.is_whitespace())
        .unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn charset_from_meta(raw: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&raw[..raw.len().min(4096)]).to_ascii_lowercase();
    let mut search = head.as_str();
    while let Some(pos) = search.find("charset") {
        let rest = &search[pos + "charset".len()..];
        if let Some(rest) = rest.trim_start().strip_prefix('=') {
            let rest = rest.trim_start().trim_start_matches(['"', '\'']);
            let end = rest
                .find(|c: char| {
                    c == ';'
                        || c == ','
                        || c == '"'
                        || c == '\''
                        || c == '/'
                        || c == '>'
                        || c.is_whitespace()
                })
                .unwrap_or(rest.len());
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        search = &search[pos + "charset".len()..];
    }
    None
}

/// Outbound proxy, from `SEOGEO_PROXY`, `HTTPS_PROXY`, or `ALL_PROXY`.
///
/// Some networks interfere with this client's TLS handshake for specific
/// hosts while leaving other tools working, which surfaces as a connection
/// reset or read timeout that looks like the site is down. A proxy is the
/// practical escape hatch. URL validation still happens locally before the
/// request, so the SSRF policy is enforced either way — but the pinned-DNS
/// guarantee does not extend through a proxy, because the proxy resolves the
/// hostname itself.
fn configured_proxy() -> Option<ureq::Proxy> {
    for var in [
        "SEOGEO_PROXY",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match ureq::Proxy::new(value) {
                Ok(p) => return Some(p),
                Err(e) => {
                    eprintln!("Warning: ignoring {var}={value:?} — {e}");
                }
            }
        }
    }
    None
}

fn agent(timeout: Duration, follow_redirects: bool, max_redirects: u32) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new();
    if let Some(proxy) = configured_proxy() {
        // With a proxy the local resolver never runs — the proxy resolves the
        // target host — so installing it would be misleading.
        builder = builder.proxy(proxy);
    } else {
        builder = builder.resolver(SafeResolver);
    }
    builder
        .timeout_connect(Duration::from_secs(15))
        // A single overall deadline. ureq propagates it into the response
        // reader, so it bounds the body as well as the handshake — setting
        // separate socket read/write timeouts on top of it makes the TLS
        // handshake fail with EAGAIN.
        .timeout(timeout)
        .redirects(if follow_redirects { max_redirects } else { 0 })
        .user_agent(DEFAULT_USER_AGENT)
        .build()
}

#[derive(Clone)]
pub struct RequestOptions {
    pub timeout: Duration,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub user_agent: Option<String>,
    pub headers: Vec<(String, String)>,
    pub max_bytes: usize,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            follow_redirects: true,
            max_redirects: 5,
            user_agent: None,
            headers: Vec::new(),
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

impl RequestOptions {
    pub fn with_timeout(secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(secs),
            ..Default::default()
        }
    }
    pub fn ua(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn max_bytes(mut self, n: usize) -> Self {
        self.max_bytes = n;
        self
    }
}

#[derive(Debug)]
pub enum HttpError {
    Safety(UrlSafetyError),
    Transport(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Safety(e) => write!(f, "url_safety: {e}"),
            HttpError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// GET a URL. Non-2xx responses are returned as `Ok` so callers can inspect
/// 404/410/5xx bodies; only transport and safety failures are `Err`.
pub fn get(url: &str, opts: &RequestOptions) -> Result<Response, HttpError> {
    request("GET", url, opts, None)
}

pub fn post_json(
    url: &str,
    body: &serde_json::Value,
    opts: &RequestOptions,
) -> Result<Response, HttpError> {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let opts = opts.clone().header("content-type", "application/json");
    request("POST", url, &opts, Some(payload))
}

pub fn post_form(
    url: &str,
    form: &[(&str, &str)],
    opts: &RequestOptions,
) -> Result<Response, HttpError> {
    let body = form
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                percent_encoding::utf8_percent_encode(k, percent_encoding::NON_ALPHANUMERIC),
                percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let opts = opts
        .clone()
        .header("content-type", "application/x-www-form-urlencoded");
    request("POST", url, &opts, Some(body.into_bytes()))
}

fn request(
    method: &str,
    url: &str,
    opts: &RequestOptions,
    body: Option<Vec<u8>>,
) -> Result<Response, HttpError> {
    let (norm, _pinned) = validate_url_strict(url).map_err(HttpError::Safety)?;
    let agent = agent(opts.timeout, opts.follow_redirects, opts.max_redirects);
    let mut req = agent.request(method, &norm);

    let ua = opts.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT);
    req = req.set("User-Agent", ua);
    req = req.set(
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    );
    req = req.set("Accept-Language", "en-US,en;q=0.9");
    for (k, v) in &opts.headers {
        req = req.set(k, v);
    }

    let outcome = match body {
        Some(b) => req.send_bytes(&b),
        None => req.call(),
    };

    let (resp, status_err) = match outcome {
        Ok(r) => (r, false),
        Err(ureq::Error::Status(_, r)) => (r, true),
        Err(ureq::Error::Transport(t)) => {
            return Err(HttpError::Transport(t.to_string()));
        }
    };

    let final_url = resp.get_url().to_string();
    let status = resp.status();
    let mut headers = BTreeMap::new();
    for name in resp.headers_names() {
        if let Some(value) = resp.header(&name) {
            headers.insert(name.to_ascii_lowercase(), value.to_string());
        }
    }

    let mut buf = Vec::new();
    // Cap the body so a hostile or accidental multi-gigabyte response cannot
    // exhaust memory; callers raise the cap deliberately for sitemap files.
    let mut reader = std::io::Read::take(resp.into_reader(), opts.max_bytes as u64 + 1);
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| HttpError::Transport(format!("read failed: {e}")))?;
    let truncated = buf.len() > opts.max_bytes;
    if truncated {
        buf.truncate(opts.max_bytes);
    }

    let response = Response {
        url: final_url,
        status,
        headers,
        body: buf,
    };

    if status_err && status >= 500 {
        // 5xx still returns the body; callers decide.
        return Ok(response);
    }
    Ok(response)
}

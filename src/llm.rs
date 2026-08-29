//! Answer-engine transport.
//!
//! Every GEO check in this binary used to stop at "here is what an answer
//! engine *would* prefer". This module is what lets it ask the engines
//! directly instead, so a citability score can be checked against the thing
//! it claims to predict.
//!
//! Keys are the caller's own (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
//! `GEMINI_API_KEY`, `PERPLEXITY_API_KEY`); nothing is proxied through a
//! service. `ollama` needs no key at all, which keeps the whole visibility
//! feature usable at zero cost.
//!
//! Only Perplexity returns real retrieval citations. The model-knowledge
//! providers answer from weights, which measures whether a brand is *known*
//! rather than whether a page was *retrieved* — a different question, and
//! [`Provider::live_search`] marks which is which so reports never conflate
//! the two.

use std::time::Instant;

use serde_json::{json, Value};

use crate::http::{self, RequestOptions};

pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    /// Environment variable holding the API key. Empty for keyless providers.
    pub env_key: &'static str,
    pub default_model: &'static str,
    /// True when the provider retrieves live web results and reports the URLs
    /// it used. False means the answer comes from model weights alone.
    pub live_search: bool,
    /// USD per million input / output tokens. Estimates for the cost ledger,
    /// not billing truth; override with `SEOGEO_LLM_PRICE_<ID>=in,out`.
    pub price_per_mtok: (f64, f64),
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "perplexity",
        label: "Perplexity (Sonar)",
        env_key: "PERPLEXITY_API_KEY",
        default_model: "sonar",
        live_search: true,
        price_per_mtok: (1.0, 1.0),
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        env_key: "OPENAI_API_KEY",
        default_model: "gpt-4o-mini",
        live_search: false,
        price_per_mtok: (0.15, 0.60),
    },
    Provider {
        id: "anthropic",
        label: "Anthropic",
        env_key: "ANTHROPIC_API_KEY",
        default_model: "claude-haiku-4-5-20251001",
        live_search: false,
        price_per_mtok: (1.0, 5.0),
    },
    Provider {
        id: "gemini",
        label: "Google Gemini",
        env_key: "GEMINI_API_KEY",
        default_model: "gemini-2.0-flash",
        live_search: false,
        price_per_mtok: (0.10, 0.40),
    },
    Provider {
        id: "ollama",
        label: "Ollama (local)",
        env_key: "",
        default_model: "llama3.2",
        live_search: false,
        price_per_mtok: (0.0, 0.0),
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

impl Provider {
    /// A provider is usable when its key is set, or when it needs no key.
    pub fn configured(&self) -> bool {
        if self.env_key.is_empty() {
            return true;
        }
        std::env::var(self.env_key).is_ok_and(|v| !v.trim().is_empty())
    }

    fn key(&self) -> String {
        std::env::var(self.env_key)
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn price(&self) -> (f64, f64) {
        let var = format!("SEOGEO_LLM_PRICE_{}", self.id.to_ascii_uppercase());
        if let Ok(raw) = std::env::var(&var) {
            let parts: Vec<f64> = raw
                .split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            if parts.len() == 2 {
                return (parts[0], parts[1]);
            }
        }
        self.price_per_mtok
    }
}

/// Providers that are configured right now, in the order listed above.
pub fn available() -> Vec<&'static Provider> {
    PROVIDERS.iter().filter(|p| p.configured()).collect()
}

/// Resolve a comma-separated provider list, defaulting to everything
/// configured. Unknown ids are an error rather than a silent skip, because a
/// typo would otherwise read as "that engine does not mention you".
pub fn resolve(spec: Option<&str>) -> Result<Vec<&'static Provider>, String> {
    let Some(spec) = spec else {
        let found = available();
        return if found.is_empty() {
            Err(format!(
                "no answer engine configured — set one of {} (or run ollama locally)",
                PROVIDERS
                    .iter()
                    .filter(|p| !p.env_key.is_empty())
                    .map(|p| p.env_key)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        } else {
            Ok(found)
        };
    };
    let mut out = Vec::new();
    for id in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match provider(id) {
            Some(p) => out.push(p),
            None => {
                return Err(format!(
                    "unknown provider {id:?} — choose from {}",
                    PROVIDERS
                        .iter()
                        .map(|p| p.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    if out.is_empty() {
        return Err("no providers selected".into());
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Probe {
    pub provider: String,
    pub model: String,
    pub live_search: bool,
    pub prompt: String,
    pub ok: bool,
    pub answer: String,
    /// URLs the engine says it used. Only ever populated for `live_search`
    /// providers; an empty list on the others is expected, not a failure.
    pub citations: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Probe {
    fn failed(p: &Provider, model: &str, prompt: &str, started: Instant, msg: String) -> Self {
        Probe {
            provider: p.id.to_string(),
            model: model.to_string(),
            live_search: p.live_search,
            prompt: prompt.to_string(),
            ok: false,
            answer: String::new(),
            citations: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            latency_ms: started.elapsed().as_millis(),
            error: Some(msg),
        }
    }
}

fn ollama_base() -> String {
    std::env::var("OLLAMA_HOST")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string())
}

/// Ask one provider one question.
///
/// Transport, HTTP, and API-level failures all come back as a `Probe` with
/// `ok: false` rather than an `Err`, because a sweep across five engines must
/// not lose four results to one bad key.
pub fn probe(p: &'static Provider, model: Option<&str>, prompt: &str, timeout: u64) -> Probe {
    let model = model.unwrap_or(p.default_model).to_string();
    let started = Instant::now();

    if !p.configured() {
        return Probe::failed(
            p,
            &model,
            prompt,
            started,
            format!("{} is not set", p.env_key),
        );
    }

    let (url, body, opts) = match p.id {
        "openai" => (
            "https://api.openai.com/v1/chat/completions".to_string(),
            json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
            }),
            api_opts(timeout).header("authorization", format!("Bearer {}", p.key())),
        ),
        "perplexity" => (
            "https://api.perplexity.ai/chat/completions".to_string(),
            json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
            }),
            api_opts(timeout).header("authorization", format!("Bearer {}", p.key())),
        ),
        "anthropic" => (
            "https://api.anthropic.com/v1/messages".to_string(),
            json!({
                "model": model,
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": prompt}],
            }),
            api_opts(timeout)
                .header("x-api-key", p.key())
                .header("anthropic-version", "2023-06-01"),
        ),
        "gemini" => (
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={}",
                http::enc(&p.key())
            ),
            json!({"contents": [{"parts": [{"text": prompt}]}]}),
            api_opts(timeout),
        ),
        "ollama" => (
            format!("{}/api/chat", ollama_base()),
            json!({
                "model": model,
                "stream": false,
                "messages": [{"role": "user", "content": prompt}],
            }),
            // The only place in the binary that leaves the SSRF resolver
            // behind. The target is the user's own model server, named by
            // them in OLLAMA_HOST and never derived from fetched content, so
            // no audited URL can reach this path.
            api_opts(timeout).allow_local(),
        ),
        other => {
            return Probe::failed(p, &model, prompt, started, format!("unsupported provider {other}"))
        }
    };

    let resp = match http::post_json(&url, &body, &opts) {
        Ok(r) => r,
        Err(e) => {
            return Probe::failed(
                p,
                &model,
                prompt,
                started,
                unreachable_hint(p, e.to_string()),
            )
        }
    };

    let text = resp.text();
    if resp.status >= 400 {
        let detail =
            extract_api_error(&text).unwrap_or_else(|| crate::output::truncate(text.trim(), 300));
        let detail = if detail.is_empty() {
            unreachable_hint(p, format!("HTTP {}", resp.status))
        } else {
            format!("HTTP {} — {detail}", resp.status)
        };
        return Probe::failed(p, &model, prompt, started, detail);
    }

    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return Probe::failed(p, &model, prompt, started, format!("bad JSON: {e}")),
    };

    let (answer, citations, tin, tout) = decode(p.id, &parsed);
    if answer.trim().is_empty() && citations.is_empty() {
        return Probe::failed(p, &model, prompt, started, "empty answer".into());
    }

    let (pin, pout) = p.price();
    let cost = (tin as f64 / 1e6) * pin + (tout as f64 / 1e6) * pout;

    Probe {
        provider: p.id.to_string(),
        model,
        live_search: p.live_search,
        prompt: prompt.to_string(),
        ok: true,
        answer,
        citations,
        input_tokens: tin,
        output_tokens: tout,
        cost_usd: crate::output::money(cost),
        latency_ms: started.elapsed().as_millis(),
        error: None,
    }
}

/// `ollama` is reported as configured whenever it is selected, because it
/// needs no key — so "cannot connect" is the common first experience, and the
/// message has to say what to do about it rather than surface a bare status.
fn unreachable_hint(p: &Provider, raw: String) -> String {
    if p.id == "ollama" {
        format!(
            "{raw} — no Ollama server answering at {}. Start one (`ollama serve`), point \
             OLLAMA_HOST at it, or choose a hosted provider with --provider.",
            ollama_base()
        )
    } else {
        raw
    }
}

fn api_opts(timeout: u64) -> RequestOptions {
    RequestOptions::with_timeout(timeout)
        .ua(http::API_USER_AGENT)
        .max_bytes(8 * 1024 * 1024)
}

/// Providers disagree on where the human-readable failure lives; try the
/// three shapes in use rather than dumping raw JSON at the user.
fn extract_api_error(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    for path in [["error", "message"], ["error", "type"]] {
        if let Some(s) = v
            .get(path[0])
            .and_then(|e| e.get(path[1]))
            .and_then(Value::as_str)
        {
            return Some(s.to_string());
        }
    }
    v.get("message").and_then(Value::as_str).map(str::to_string)
}

/// Pull answer text, citations, and token counts out of each provider's
/// response shape.
fn decode(id: &str, v: &Value) -> (String, Vec<String>, u64, u64) {
    let u = |p: &str, k: &str| {
        v.get(p)
            .and_then(|x| x.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    match id {
        "openai" | "perplexity" => {
            let answer = v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            // Sonar reports sources as a bare `citations` array on older
            // responses and as `search_results[].url` on newer ones.
            let mut cites = string_list(&v["citations"]);
            if cites.is_empty() {
                cites = v["search_results"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r.get("url").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            (
                answer,
                cites,
                u("usage", "prompt_tokens"),
                u("usage", "completion_tokens"),
            )
        }
        "anthropic" => {
            let answer = v["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            (
                answer,
                Vec::new(),
                u("usage", "input_tokens"),
                u("usage", "output_tokens"),
            )
        }
        "gemini" => {
            let answer = v["candidates"][0]["content"]["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let cites = v["candidates"][0]["groundingMetadata"]["groundingChunks"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| {
                            c.get("web")
                                .and_then(|w| w.get("uri"))
                                .and_then(Value::as_str)
                        })
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            (
                answer,
                cites,
                u("usageMetadata", "promptTokenCount"),
                u("usageMetadata", "candidatesTokenCount"),
            )
        }
        "ollama" => {
            let answer = v["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (
                answer,
                Vec::new(),
                v["prompt_eval_count"].as_u64().unwrap_or(0),
                v["eval_count"].as_u64().unwrap_or(0),
            )
        }
        _ => (String::new(), Vec::new(), 0, 0),
    }
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Registrable domain-ish key for grouping citation URLs, with `www.` folded
/// in so `www.example.com` and `example.com` are one row in a leaderboard.
pub fn cite_domain(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed
        .host_str()?
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Same normalisation for a value the user typed, which may be a bare host,
/// a host with `www.`, or a full URL.
pub fn cite_domain_of_host(input: &str) -> Option<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains("://") {
        return cite_domain(raw);
    }
    cite_domain(&format!("https://{raw}"))
}

/// Default per-call timeout. Live-search providers routinely take 20s+.
pub const DEFAULT_TIMEOUT: u64 = 90;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_openai_shape() {
        let v = json!({
            "choices": [{"message": {"content": "Acme is a CDN."}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let (a, c, i, o) = decode("openai", &v);
        assert_eq!(a, "Acme is a CDN.");
        assert!(c.is_empty());
        assert_eq!((i, o), (10, 5));
    }

    #[test]
    fn decodes_perplexity_search_results_fallback() {
        let v = json!({
            "choices": [{"message": {"content": "See below."}}],
            "search_results": [{"url": "https://a.test/x"}, {"url": "https://b.test/y"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let (_, c, _, _) = decode("perplexity", &v);
        assert_eq!(c, vec!["https://a.test/x", "https://b.test/y"]);
    }

    #[test]
    fn decodes_anthropic_multiblock_text() {
        let v = json!({
            "content": [{"type": "text", "text": "one "}, {"type": "text", "text": "two"}],
            "usage": {"input_tokens": 7, "output_tokens": 3}
        });
        let (a, _, i, o) = decode("anthropic", &v);
        assert_eq!(a, "one two");
        assert_eq!((i, o), (7, 3));
    }

    #[test]
    fn decodes_gemini_grounding_citations() {
        let v = json!({
            "candidates": [{
                "content": {"parts": [{"text": "hi"}]},
                "groundingMetadata": {"groundingChunks": [{"web": {"uri": "https://g.test/1"}}]}
            }],
            "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 6}
        });
        let (a, c, i, o) = decode("gemini", &v);
        assert_eq!(a, "hi");
        assert_eq!(c, vec!["https://g.test/1"]);
        assert_eq!((i, o), (4, 6));
    }

    #[test]
    fn cite_domain_folds_www() {
        assert_eq!(
            cite_domain("https://www.Example.com/a"),
            Some("example.com".into())
        );
        assert_eq!(cite_domain("not a url"), None);
    }

    #[test]
    fn unknown_provider_is_an_error_not_a_skip() {
        assert!(resolve(Some("openai,nope")).is_err());
    }
}

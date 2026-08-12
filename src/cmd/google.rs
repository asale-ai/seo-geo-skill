//! Google API commands.
//!
//! Credentials come from `~/.config/seogeo/google-api.json` with environment
//! variable fallbacks, so nothing sensitive lives in the repo. Two auth
//! modes are supported: an API key (PageSpeed, CrUX, Natural Language,
//! YouTube) and a service account signed into an OAuth2 access token
//! (Search Console, Indexing, GA4).

use std::collections::BTreeMap;
use std::process::ExitCode;

use base64::Engine;
use serde_json::{json, Value};

use crate::cli::{FormFactor, KeywordAction, Strategy, YoutubeAction};
use crate::http::{self, RequestOptions};
use crate::output::{days_ago, err, print_json, today_utc, truncate, CmdResult, Error};
use crate::safety::{coerce_scheme, validate_url_strict};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

const PSI_ENDPOINT: &str = "https://www.googleapis.com/pagespeedonline/v5/runPagespeed";
const CRUX_RECORD: &str = "https://chromeuxreport.googleapis.com/v1/records:queryRecord";
const CRUX_HISTORY: &str = "https://chromeuxreport.googleapis.com/v1/records:queryHistoryRecord";
const NLP_ENDPOINT: &str = "https://language.googleapis.com/v1/documents:annotateText";
const YOUTUBE_API: &str = "https://www.googleapis.com/youtube/v3";
const GSC_API: &str = "https://www.googleapis.com/webmasters/v3";
const GSC_INSPECT_API: &str = "https://searchconsole.googleapis.com/v1/urlInspection/index:inspect";
const INDEXING_API: &str = "https://indexing.googleapis.com/v3";
const GA4_API: &str = "https://analyticsdata.googleapis.com/v1beta";
const ADS_API: &str = "https://googleads.googleapis.com/v18";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

const SCOPE_GSC: &str = "https://www.googleapis.com/auth/webmasters.readonly";
const SCOPE_INDEXING: &str = "https://www.googleapis.com/auth/indexing";
const SCOPE_GA4: &str = "https://www.googleapis.com/auth/analytics.readonly";

pub fn config_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("google-api.json")
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub service_account_path: Option<String>,
    pub default_property: Option<String>,
    pub ga4_property_id: Option<String>,
    pub ads_developer_token: Option<String>,
    pub ads_customer_id: Option<String>,
    pub indexnow_key: Option<String>,
}

fn env_or(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

pub fn load_config() -> Config {
    let mut cfg = Config::default();
    if let Ok(raw) = std::fs::read_to_string(config_path()) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let take = |k: &str| {
                v[k].as_str()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
            };
            cfg.api_key = take("api_key");
            cfg.service_account_path = take("service_account_path");
            cfg.default_property = take("default_property");
            cfg.ga4_property_id = take("ga4_property_id");
            cfg.ads_developer_token = take("ads_developer_token");
            cfg.ads_customer_id = take("ads_customer_id");
            cfg.indexnow_key = take("indexnow_key");
        }
    }
    cfg.api_key = cfg.api_key.or_else(|| env_or("GOOGLE_API_KEY"));
    cfg.service_account_path = cfg
        .service_account_path
        .or_else(|| env_or("GOOGLE_APPLICATION_CREDENTIALS"));
    cfg.default_property = cfg.default_property.or_else(|| env_or("GSC_PROPERTY"));
    cfg.ga4_property_id = cfg.ga4_property_id.or_else(|| env_or("GA4_PROPERTY_ID"));
    cfg.ads_developer_token = cfg
        .ads_developer_token
        .or_else(|| env_or("GOOGLE_ADS_DEVELOPER_TOKEN"));
    cfg.ads_customer_id = cfg
        .ads_customer_id
        .or_else(|| env_or("GOOGLE_ADS_CUSTOMER_ID"));
    cfg.indexnow_key = cfg.indexnow_key.or_else(|| env_or("INDEXNOW_KEY"));
    cfg
}

pub fn api_key() -> Option<String> {
    load_config().api_key
}

/// Google API keys look like `AIza…`; strip them from anything we print so a
/// key never lands in a log or an agent transcript.
pub fn redact(text: &str) -> String {
    let re = regex::Regex::new(r"AIza[0-9A-Za-z_\-]+").unwrap();
    let text = re.replace_all(text, "GOOGLE_API_KEY_REDACTED");
    let qre = regex::Regex::new(r#"([?&])key=[^&\s'"<>)]*"#).unwrap();
    qre.replace_all(&text, "${1}key=REDACTED").into_owned()
}

fn key_opts(api_key: &str, timeout: u64) -> RequestOptions {
    RequestOptions::with_timeout(timeout).header("X-Goog-Api-Key", api_key)
}

fn require_key(cfg: &Config) -> CmdResult<String> {
    cfg.api_key.clone().ok_or_else(|| {
        Error(
            "Google API key not configured. Set GOOGLE_API_KEY, or run \
             `seogeo google-auth --setup` for the config-file layout."
                .into(),
        )
    })
}

// -------------------------------------------------------- service account

#[derive(Debug)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
    token_uri: String,
}

fn load_service_account(path: &str) -> CmdResult<ServiceAccount> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| Error(format!("could not read service account {path}: {e}")))?;
    let v: Value = serde_json::from_str(&raw)?;
    let client_email = v["client_email"]
        .as_str()
        .ok_or_else(|| Error("service account JSON has no client_email".into()))?
        .to_string();
    let private_key = v["private_key"]
        .as_str()
        .ok_or_else(|| Error("service account JSON has no private_key".into()))?
        .to_string();
    let token_uri = v["token_uri"].as_str().unwrap_or(TOKEN_URI).to_string();
    Ok(ServiceAccount {
        client_email,
        private_key,
        token_uri,
    })
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sign a service-account JWT and exchange it for an access token.
fn access_token(scopes: &[&str]) -> CmdResult<String> {
    let cfg = load_config();
    let path = cfg.service_account_path.ok_or_else(|| {
        Error(
            "No service account configured. Set GOOGLE_APPLICATION_CREDENTIALS to a service \
             account JSON with access to this API, or add `service_account_path` to the config."
                .into(),
        )
    })?;
    let sa = load_service_account(&path)?;

    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let header = json!({"alg": "RS256", "typ": "JWT"});
    let claims = json!({
        "iss": sa.client_email,
        "scope": scopes.join(" "),
        "aud": sa.token_uri,
        "iat": now,
        "exp": now + 3600,
    });
    let signing_input = format!(
        "{}.{}",
        b64url(serde_json::to_string(&header)?.as_bytes()),
        b64url(serde_json::to_string(&claims)?.as_bytes())
    );

    let signature = {
        use rsa::pkcs1v15::SigningKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::signature::{SignatureEncoding, Signer};
        use sha2::Sha256;

        let key = rsa::RsaPrivateKey::from_pkcs8_pem(&sa.private_key).map_err(|e| {
            Error(format!(
                "service account private key is not valid PKCS#8: {e}"
            ))
        })?;
        let signer = SigningKey::<Sha256>::new(key);
        b64url(&signer.sign(signing_input.as_bytes()).to_bytes())
    };

    let assertion = format!("{signing_input}.{signature}");
    let resp = http::post_form(
        &sa.token_uri,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &assertion),
        ],
        &RequestOptions::with_timeout(30),
    )?;
    let body: Value = serde_json::from_slice(&resp.body).map_err(|_| {
        Error(format!(
            "token endpoint returned non-JSON: {}",
            redact(&resp.text())
        ))
    })?;
    body["access_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            Error(format!(
                "token exchange failed: {}",
                redact(&body.to_string())
            ))
        })
}

fn bearer_opts(token: &str, timeout: u64) -> RequestOptions {
    RequestOptions::with_timeout(timeout).header("Authorization", format!("Bearer {token}"))
}

fn json_call(
    method: &str,
    url: &str,
    body: Option<&Value>,
    opts: &RequestOptions,
) -> CmdResult<Value> {
    let resp = match (method, body) {
        ("POST", Some(b)) => http::post_json(url, b, opts)?,
        ("POST", None) => http::post_json(url, &json!({}), opts)?,
        _ => http::get(url, opts)?,
    };
    let text = resp.text();
    if !(200..300).contains(&resp.status) {
        return err(format!(
            "HTTP {}: {}",
            resp.status,
            redact(&truncate(&text, 600))
        ));
    }
    serde_json::from_str(&text)
        .map_err(|e| Error(format!("invalid JSON from {}: {e}", redact(url))))
}

// ------------------------------------------------------------------- auth

pub fn auth(check: Option<&str>, setup: bool, tier: bool, json: bool) -> CmdResult<ExitCode> {
    if setup {
        print_setup();
        return OK;
    }
    let cfg = load_config();
    let has_key = cfg.api_key.is_some();
    let has_sa = cfg
        .service_account_path
        .as_deref()
        .map(|p| std::path::Path::new(p).exists())
        .unwrap_or(false);

    let tier_name = match (has_key, has_sa) {
        (true, true) => "full",
        (false, true) => "authenticated",
        (true, false) => "field-data",
        (false, false) => "none",
    };

    if tier {
        let out = json!({
            "tier": tier_name,
            "api_key": has_key,
            "service_account": has_sa,
        });
        if json {
            print_json(&out)?;
        } else {
            println!("Tier: {tier_name}");
        }
        return OK;
    }

    let services: BTreeMap<&str, (&str, bool, &str)> = BTreeMap::from([
        ("psi", ("PageSpeed Insights v5", has_key, "api_key")),
        ("crux", ("Chrome UX Report", has_key, "api_key")),
        ("crux_history", ("CrUX History API", has_key, "api_key")),
        ("nlp", ("Cloud Natural Language", has_key, "api_key")),
        ("youtube", ("YouTube Data API v3", has_key, "api_key")),
        ("gsc", ("Search Console API", has_sa, "service_account")),
        ("indexing", ("Indexing API v3", has_sa, "service_account")),
        (
            "ga4",
            (
                "GA4 Data API",
                has_sa && cfg.ga4_property_id.is_some(),
                "service_account",
            ),
        ),
        (
            "ads",
            (
                "Google Ads Keyword Planner",
                cfg.ads_developer_token.is_some() && cfg.ads_customer_id.is_some() && has_sa,
                "service_account+developer_token",
            ),
        ),
    ]);

    let wanted = check.unwrap_or("all");
    let mut results = serde_json::Map::new();
    for (key, (name, ok, method)) in &services {
        if wanted != "all" && wanted != *key {
            continue;
        }
        results.insert(
            key.to_string(),
            json!({
                "service": name,
                "configured": ok,
                "method": method,
                "hint": if *ok { Value::Null } else {
                    json!(format!("run `seogeo google-auth --setup` and configure {method}"))
                },
            }),
        );
    }
    if results.is_empty() {
        return err(format!("unknown service {wanted:?}"));
    }

    let out = json!({
        "config_path": config_path().display().to_string(),
        "tier": tier_name,
        "services": results,
    });
    let all_ok = out["services"]
        .as_object()
        .unwrap()
        .values()
        .all(|v| v["configured"].as_bool().unwrap_or(false));

    if json {
        print_json(&out)?;
    } else {
        println!("Config: {}", config_path().display());
        println!("Tier:   {tier_name}");
        for (k, v) in out["services"].as_object().unwrap() {
            println!(
                "  [{}] {:<14} {}",
                if v["configured"].as_bool().unwrap_or(false) {
                    "OK "
                } else {
                    "-- "
                },
                k,
                v["service"].as_str().unwrap_or("")
            );
        }
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn print_setup() {
    println!(
        r#"Google API setup for seogeo
===========================

1. Create a Google Cloud project: https://console.cloud.google.com

2. API key (PageSpeed, CrUX, Natural Language, YouTube)
   - APIs & Services -> Credentials -> Create credentials -> API key
   - Enable: PageSpeed Insights API, Chrome UX Report API,
     Cloud Natural Language API, YouTube Data API v3
   - Export it:  export GOOGLE_API_KEY=...

3. Service account (Search Console, Indexing API, GA4)
   - IAM & Admin -> Service Accounts -> Create -> add a JSON key
   - Search Console: add the service account email as a user on the property
   - GA4: add the email as a Viewer on the property
   - Indexing API: add the email as an Owner in Search Console
   - Export it:  export GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json

4. Optional config file at {path}

   {{
     "api_key": "<GOOGLE_API_KEY>",
     "service_account_path": "/path/to/service-account.json",
     "default_property": "https://example.com/",
     "ga4_property_id": "123456789",
     "ads_developer_token": "<token>",
     "ads_customer_id": "1234567890"
   }}

Never commit any of these files. Verify with `seogeo google-auth --check`.
"#,
        path = config_path().display()
    );
}

// -------------------------------------------------------------- pagespeed

pub fn pagespeed(
    url: &str,
    strategy: Strategy,
    psi_only: bool,
    crux_only: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    let (norm, _) = validate_url_strict(&coerce_scheme(url))?;
    let cfg = load_config();
    // PageSpeed Insights serves keyless requests at a low quota, so a
    // Lighthouse run works with zero setup. CrUX does not — it needs a key.
    let key = cfg.api_key.clone();
    if key.is_none() && crux_only {
        return Err(require_key(&cfg).unwrap_err());
    }
    if key.is_none() {
        eprintln!(
            "Note: no GOOGLE_API_KEY set. Running PageSpeed keyless (low shared quota); \
             CrUX field data is skipped. Run `seogeo google-auth --setup` to add a key."
        );
    }

    let strategies: Vec<&str> = match strategy {
        Strategy::Mobile => vec!["mobile"],
        Strategy::Desktop => vec!["desktop"],
        Strategy::Both => vec!["mobile", "desktop"],
    };

    let mut psi = serde_json::Map::new();
    if !crux_only {
        for s in &strategies {
            let endpoint = format!(
                "{PSI_ENDPOINT}?url={}&strategy={s}\
                 &category=performance&category=seo&category=accessibility&category=best-practices",
                http::enc(&norm)
            );
            let opts = match &key {
                Some(k) => key_opts(k, 120),
                None => RequestOptions::with_timeout(120),
            };
            // PSI runs a full Lighthouse pass server-side; it regularly needs
            // more than a minute for a heavy page.
            match json_call("GET", &endpoint, None, &opts) {
                Ok(v) => psi.insert(s.to_string(), summarize_psi(&v)),
                Err(e) => psi.insert(s.to_string(), json!({"error": e.to_string()})),
            };
        }
    }

    let crux = match (&key, psi_only) {
        (_, true) => Value::Null,
        (None, _) => json!({"error": "CrUX field data needs GOOGLE_API_KEY"}),
        (Some(k), _) => match crux_record(&norm, "PHONE", k) {
            Ok(v) => v,
            Err(e) => json!({"error": e.to_string()}),
        },
    };

    let result = json!({"url": norm, "psi": psi, "crux": crux});

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {norm}");
        for (s, data) in result["psi"].as_object().unwrap() {
            if let Some(e) = data["error"].as_str() {
                println!("  {s}: error — {e}");
                continue;
            }
            println!("  {s}:");
            for (k, v) in data["lighthouse_scores"].as_object().unwrap() {
                println!("    {k:<16} {v}");
            }
            for (k, v) in data["lab_metrics"].as_object().unwrap() {
                println!("    {k:<16} {v}");
            }
        }
        if let Some(metrics) = result["crux"].as_object() {
            println!("  CrUX field (p75):");
            for (k, v) in metrics {
                println!("    {k:<40} {}", v["p75"]);
            }
        }
    }
    OK
}

fn summarize_psi(raw: &Value) -> Value {
    let categories = &raw["lighthouseResult"]["categories"];
    let audits = &raw["lighthouseResult"]["audits"];
    let score = |name: &str| {
        categories[name]["score"]
            .as_f64()
            .map(|s| (s * 100.0).round())
    };
    let numeric = |name: &str| audits[name]["numericValue"].as_f64();

    json!({
        "lighthouse_scores": {
            "performance": score("performance"),
            "seo": score("seo"),
            "accessibility": score("accessibility"),
            "best_practices": score("best-practices"),
        },
        "lab_metrics": {
            "first_contentful_paint_ms": numeric("first-contentful-paint"),
            "largest_contentful_paint_ms": numeric("largest-contentful-paint"),
            "total_blocking_time_ms": numeric("total-blocking-time"),
            "cumulative_layout_shift": numeric("cumulative-layout-shift"),
            "speed_index_ms": numeric("speed-index"),
            "time_to_first_byte_ms": numeric("server-response-time"),
        },
        "opportunities": audits.as_object().map(|a| {
            let mut items: Vec<Value> = a.iter()
                .filter(|(_, v)| v["details"]["type"] == "opportunity")
                .filter_map(|(k, v)| {
                    let saving = v["details"]["overallSavingsMs"].as_f64()?;
                    (saving > 0.0).then(|| json!({
                        "id": k,
                        "title": v["title"],
                        "savings_ms": saving,
                    }))
                })
                .collect();
            items.sort_by(|a, b| b["savings_ms"].as_f64().partial_cmp(&a["savings_ms"].as_f64()).unwrap());
            items.truncate(10);
            Value::Array(items)
        }),
    })
}

/// Query CrUX for the standard field metrics; returns `{metric: {p75, ...}}`.
pub fn crux_record(url: &str, form_factor: &str, key: &str) -> CmdResult<Value> {
    let body = json!({"url": url, "formFactor": form_factor});
    let raw = json_call("POST", CRUX_RECORD, Some(&body), &key_opts(key, 30))?;
    let metrics = &raw["record"]["metrics"];
    let mut out = serde_json::Map::new();
    if let Some(map) = metrics.as_object() {
        for (name, data) in map {
            out.insert(
                name.clone(),
                json!({
                    "p75": data["percentiles"]["p75"],
                    "histogram": data["histogram"],
                }),
            );
        }
    }
    Ok(Value::Object(out))
}

// ------------------------------------------------------------ crux history

pub fn crux_history(
    url: &str,
    form_factor: FormFactor,
    origin: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    let (norm, _) = validate_url_strict(&coerce_scheme(url))?;
    let key = require_key(&load_config())?;

    let target = if origin {
        let parsed = url::Url::parse(&norm).map_err(|e| Error(e.to_string()))?;
        json!({"origin": format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""))})
    } else {
        json!({"url": norm})
    };
    let mut body = target;
    body["formFactor"] = json!(form_factor.as_api());

    let raw = json_call("POST", CRUX_HISTORY, Some(&body), &key_opts(&key, 45))?;
    let record = &raw["record"];
    let periods = record["collectionPeriods"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut series = serde_json::Map::new();
    if let Some(metrics) = record["metrics"].as_object() {
        for (name, data) in metrics {
            let p75 = data["percentilesTimeseries"]["p75s"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // The last period with data is the current state; the first is
            // ~25 weeks back. Trend = latest vs earliest non-null.
            let numeric: Vec<Option<f64>> = p75
                .iter()
                .map(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .collect();
            let first = numeric.iter().flatten().next().copied();
            let last = numeric.iter().flatten().next_back().copied();
            let trend = match (first, last) {
                (Some(f), Some(l)) if f > 0.0 => {
                    let pct = (l - f) / f * 100.0;
                    json!({
                        "first": f, "last": l,
                        "change_pct": (pct * 10.0).round() / 10.0,
                        "direction": if pct > 5.0 { "worse" } else if pct < -5.0 { "better" } else { "flat" },
                    })
                }
                _ => Value::Null,
            };
            series.insert(name.clone(), json!({"p75_timeseries": p75, "trend": trend}));
        }
    }

    let result = json!({
        "url": norm,
        "form_factor": form_factor.as_api(),
        "collection_periods": periods.len(),
        "metrics": series,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {norm} ({} weeks)", periods.len());
        for (name, data) in result["metrics"].as_object().unwrap() {
            println!(
                "  {name:<40} {} ({})",
                data["trend"]["last"],
                data["trend"]["direction"].as_str().unwrap_or("n/a")
            );
        }
    }
    OK
}

// ----------------------------------------------------------- LCP subparts

const LCP_SUBPART_METRICS: &[&str] = &[
    "largest_contentful_paint_image_time_to_first_byte",
    "largest_contentful_paint_image_resource_load_delay",
    "largest_contentful_paint_image_resource_load_duration",
    "largest_contentful_paint_image_element_render_delay",
];

/// Decompose LCP into network, scheduling, fetch, and render phases. This is
/// what turns "your LCP is 4.2s" into "your TTFB is 1.1s and your render
/// delay is 2.4s", which is the difference between a number and a task.
pub fn lcp_subparts(url: &str, form_factor: FormFactor, json: bool) -> CmdResult<ExitCode> {
    let (norm, _) = validate_url_strict(&coerce_scheme(url))?;
    let key = require_key(&load_config())?;

    let mut metrics: Vec<&str> = LCP_SUBPART_METRICS.to_vec();
    metrics.push("largest_contentful_paint");
    let body = json!({
        "url": norm,
        "formFactor": form_factor.as_api(),
        "metrics": metrics,
    });
    let raw = json_call("POST", CRUX_RECORD, Some(&body), &key_opts(&key, 30))?;
    let record = &raw["record"]["metrics"];

    let p75 = |name: &str| -> Option<f64> {
        let v = &record[name]["percentiles"]["p75"];
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };

    let mut breakdown = serde_json::Map::new();
    for m in LCP_SUBPART_METRICS {
        breakdown.insert(m.to_string(), json!(p75(m)));
    }
    let overall = p75("largest_contentful_paint");

    let mut dominant = Vec::new();
    if let Some(total) = overall.filter(|t| *t > 0.0) {
        for m in LCP_SUBPART_METRICS {
            if let Some(v) = p75(m) {
                let share = v / total;
                if share >= 0.40 {
                    dominant.push(json!({
                        "metric": m,
                        "p75_ms": v,
                        "share": (share * 100.0).round() / 100.0,
                    }));
                }
            }
        }
    }

    let recommendations: Vec<String> = dominant
        .iter()
        .filter_map(|d| {
            let m = d["metric"].as_str()?;
            Some(if m.ends_with("time_to_first_byte") {
                "TTFB dominates LCP. Check origin response time, server-side compute, and CDN \
                 edge cache hit rate. Aim for TTFB under 0.8s."
            } else if m.ends_with("resource_load_delay") {
                "Resource load delay dominates. The LCP element is discovered late; preload the \
                 hero image with fetchpriority=high or move it ahead of blocking resources."
            } else if m.ends_with("resource_load_duration") {
                "Resource load duration dominates. The LCP image is large. Serve responsive sizes \
                 (srcset), modern formats (AVIF/WebP), and async decoding hints."
            } else {
                "Element render delay dominates. The element is loaded but painting is blocked. \
                 Reduce render-blocking CSS/JS above the fold and avoid font-blocking shifts."
            }
            .to_string())
        })
        .collect();

    let result = json!({
        "url": norm,
        "form_factor": form_factor.as_api(),
        "p75_lcp_ms": overall,
        "subparts_p75_ms": breakdown,
        "dominant_subparts": dominant,
        "recommendations": recommendations,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("URL: {norm}");
        println!("Form factor: {}", form_factor.as_api());
        println!("Overall p75 LCP: {} ms", result["p75_lcp_ms"]);
        println!("Subparts (p75 ms):");
        for (k, v) in result["subparts_p75_ms"].as_object().unwrap() {
            let label = k.replace("largest_contentful_paint_image_", "");
            println!("  {label:<35} {v}");
        }
        if !dominant.is_empty() {
            println!("\nDominant subparts (>= 40% of LCP):");
            for d in &dominant {
                println!(
                    "  {} = {} ms ({:.0}%)",
                    d["metric"]
                        .as_str()
                        .unwrap_or("")
                        .replace("largest_contentful_paint_image_", ""),
                    d["p75_ms"],
                    d["share"].as_f64().unwrap_or(0.0) * 100.0
                );
            }
        }
        for r in &recommendations {
            println!("  - {r}");
        }
    }
    OK
}

// -------------------------------------------------------------------- GSC

fn gsc_property(explicit: Option<&str>) -> CmdResult<String> {
    explicit
        .map(|s| s.to_string())
        .or_else(|| load_config().default_property)
        .ok_or_else(|| {
            Error(
                "No Search Console property. Pass --property or set GSC_PROPERTY \
                 (e.g. https://example.com/ or sc-domain:example.com)."
                    .into(),
            )
        })
}

fn encode_property(property: &str) -> String {
    http::enc(property)
}

#[allow(clippy::too_many_arguments)]
pub fn gsc_query(
    property: Option<&str>,
    dimensions: &str,
    days: i64,
    start_date: Option<&str>,
    end_date: Option<&str>,
    search_type: &str,
    limit: u32,
    country: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let property = gsc_property(property)?;
    let token = access_token(&[SCOPE_GSC])?;

    let dims: Vec<&str> = dimensions
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .collect();
    const VALID: &[&str] = &[
        "query",
        "page",
        "country",
        "device",
        "searchAppearance",
        "date",
    ];
    for d in &dims {
        if !VALID.contains(d) {
            return err(format!(
                "invalid dimension {d:?}; expected one of {}",
                VALID.join(", ")
            ));
        }
    }

    // GSC data lags ~2 days, so the default window ends there rather than today.
    let end = end_date
        .map(|s| s.to_string())
        .unwrap_or_else(|| days_ago(2));
    let start = start_date
        .map(|s| s.to_string())
        .unwrap_or_else(|| days_ago(days + 2));

    let mut body = json!({
        "startDate": start,
        "endDate": end,
        "dimensions": dims,
        "type": search_type,
        "rowLimit": limit.min(25_000),
    });
    if let Some(c) = country {
        body["dimensionFilterGroups"] = json!([{
            "filters": [{"dimension": "country", "operator": "equals", "expression": c.to_lowercase()}]
        }]);
    }

    let url = format!(
        "{GSC_API}/sites/{}/searchAnalytics/query",
        encode_property(&property)
    );
    let raw = json_call("POST", &url, Some(&body), &bearer_opts(&token, 60))?;

    let rows: Vec<Value> = raw["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let keys = r["keys"].as_array().cloned().unwrap_or_default();
            let mut obj = serde_json::Map::new();
            for (i, d) in dims.iter().enumerate() {
                obj.insert(d.to_string(), keys.get(i).cloned().unwrap_or(Value::Null));
            }
            obj.insert("clicks".into(), r["clicks"].clone());
            obj.insert("impressions".into(), r["impressions"].clone());
            obj.insert("ctr".into(), r["ctr"].clone());
            obj.insert("position".into(), r["position"].clone());
            Value::Object(obj)
        })
        .collect();

    let totals = json!({
        "clicks": rows.iter().filter_map(|r| r["clicks"].as_f64()).sum::<f64>(),
        "impressions": rows.iter().filter_map(|r| r["impressions"].as_f64()).sum::<f64>(),
    });

    let result = json!({
        "property": property,
        "start_date": start,
        "end_date": end,
        "dimensions": dims,
        "row_count": rows.len(),
        "totals": totals,
        "rows": rows,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("{property}  {start} .. {end}  ({} rows)", rows.len());
        println!(
            "Totals: {} clicks, {} impressions",
            totals["clicks"], totals["impressions"]
        );
        for r in rows.iter().take(25) {
            let label: Vec<String> = dims
                .iter()
                .map(|d| r[*d].as_str().unwrap_or("").to_string())
                .collect();
            println!(
                "  {:<50} clicks={:<6} impr={:<8} ctr={:.2}% pos={:.1}",
                truncate(&label.join(" | "), 50),
                r["clicks"],
                r["impressions"],
                r["ctr"].as_f64().unwrap_or(0.0) * 100.0,
                r["position"].as_f64().unwrap_or(0.0)
            );
        }
    }
    OK
}

pub fn gsc_sitemaps(property: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let property = gsc_property(property)?;
    let token = access_token(&[SCOPE_GSC])?;
    let url = format!("{GSC_API}/sites/{}/sitemaps", encode_property(&property));
    let raw = json_call("GET", &url, None, &bearer_opts(&token, 30))?;

    let sitemaps: Vec<Value> = raw["sitemap"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            json!({
                "path": s["path"],
                "last_submitted": s["lastSubmitted"],
                "last_downloaded": s["lastDownloaded"],
                "is_pending": s["isPending"],
                "errors": s["errors"],
                "warnings": s["warnings"],
                "contents": s["contents"],
            })
        })
        .collect();

    let result = json!({"property": property, "count": sitemaps.len(), "sitemaps": sitemaps});
    if json {
        print_json(&result)?;
    } else {
        println!("{property}: {} sitemap(s)", sitemaps.len());
        for s in &sitemaps {
            println!(
                "  {}  errors={} warnings={} pending={}",
                s["path"].as_str().unwrap_or(""),
                s["errors"],
                s["warnings"],
                s["is_pending"]
            );
        }
    }
    OK
}

pub fn gsc_sites(json: bool) -> CmdResult<ExitCode> {
    let token = access_token(&[SCOPE_GSC])?;
    let raw = json_call(
        "GET",
        &format!("{GSC_API}/sites"),
        None,
        &bearer_opts(&token, 30),
    )?;
    let sites: Vec<Value> = raw["siteEntry"].as_array().cloned().unwrap_or_default();
    let result = json!({"count": sites.len(), "sites": sites});
    if json {
        print_json(&result)?;
    } else {
        for s in &sites {
            println!(
                "  {:<50} {}",
                s["siteUrl"].as_str().unwrap_or(""),
                s["permissionLevel"].as_str().unwrap_or("")
            );
        }
    }
    OK
}

pub fn gsc_inspect(
    url: Option<&str>,
    batch: Option<&str>,
    property: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let property = gsc_property(property)?;
    let token = access_token(&[SCOPE_GSC])?;

    let targets: Vec<String> = match (batch, url) {
        (Some(path), _) => std::fs::read_to_string(path)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
        (None, Some(u)) => vec![u.to_string()],
        (None, None) => return err("pass a URL or --batch <file>"),
    };

    let mut results = Vec::new();
    for target in &targets {
        let body = json!({
            "inspectionUrl": target,
            "siteUrl": property,
            "languageCode": "en-US",
        });
        match json_call(
            "POST",
            GSC_INSPECT_API,
            Some(&body),
            &bearer_opts(&token, 60),
        ) {
            Ok(raw) => {
                let index = &raw["inspectionResult"]["indexStatusResult"];
                results.push(json!({
                    "url": target,
                    "verdict": index["verdict"],
                    "coverage_state": index["coverageState"],
                    "robots_txt_state": index["robotsTxtState"],
                    "indexing_state": index["indexingState"],
                    "last_crawl_time": index["lastCrawlTime"],
                    "page_fetch_state": index["pageFetchState"],
                    "google_canonical": index["googleCanonical"],
                    "user_canonical": index["userCanonical"],
                    "crawled_as": index["crawledAs"],
                    "rich_results": raw["inspectionResult"]["richResultsResult"],
                    "mobile_usability": raw["inspectionResult"]["mobileUsabilityResult"],
                }));
            }
            Err(e) => results.push(json!({"url": target, "error": e.to_string()})),
        }
    }

    let result = json!({"property": property, "count": results.len(), "results": results});
    if json {
        print_json(&result)?;
    } else {
        for r in &results {
            println!(
                "  {:<60} {} / {}",
                truncate(r["url"].as_str().unwrap_or(""), 60),
                r["verdict"].as_str().unwrap_or("?"),
                r["coverage_state"].as_str().unwrap_or("?")
            );
        }
    }
    OK
}

// --------------------------------------------------------------- indexing

pub fn indexing_notify(
    url: Option<&str>,
    batch: Option<&str>,
    notification_type: &str,
    status: Option<&str>,
    json: bool,
) -> CmdResult<ExitCode> {
    let token = access_token(&[SCOPE_INDEXING])?;

    if let Some(target) = status {
        let endpoint = format!(
            "{INDEXING_API}/urlNotifications/metadata?url={}",
            http::enc(target)
        );
        let raw = json_call("GET", &endpoint, None, &bearer_opts(&token, 30))?;
        print_json(&raw)?;
        return OK;
    }

    if !matches!(notification_type, "URL_UPDATED" | "URL_DELETED") {
        return err("--type must be URL_UPDATED or URL_DELETED");
    }

    let targets: Vec<String> = match (batch, url) {
        (Some(path), _) => std::fs::read_to_string(path)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
        (None, Some(u)) => vec![u.to_string()],
        (None, None) => return err("pass a URL, --batch <file>, or --status <url>"),
    };

    let mut results = Vec::new();
    for target in &targets {
        let body = json!({"url": target, "type": notification_type});
        match json_call(
            "POST",
            &format!("{INDEXING_API}/urlNotifications:publish"),
            Some(&body),
            &bearer_opts(&token, 30),
        ) {
            Ok(raw) => results.push(json!({"url": target, "ok": true, "response": raw})),
            Err(e) => results.push(json!({"url": target, "ok": false, "error": e.to_string()})),
        }
    }

    let ok_count = results.iter().filter(|r| r["ok"] == true).count();
    let result = json!({
        "type": notification_type,
        "submitted": results.len(),
        "succeeded": ok_count,
        "results": results,
    });
    if json {
        print_json(&result)?;
    } else {
        println!("{ok_count}/{} notifications accepted", results.len());
        for r in &results {
            if r["ok"] != true {
                println!("  FAIL {} — {}", r["url"], r["error"]);
            }
        }
    }
    Ok(if ok_count == results.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// -------------------------------------------------------------------- GA4

pub fn ga4_report(
    property: Option<&str>,
    report: &str,
    days: i64,
    limit: u32,
    json: bool,
) -> CmdResult<ExitCode> {
    let cfg = load_config();
    let property = property
        .map(|s| s.to_string())
        .or(cfg.ga4_property_id)
        .ok_or_else(|| Error("No GA4 property. Pass --property or set GA4_PROPERTY_ID.".into()))?;
    let property_id = property.trim_start_matches("properties/").to_string();
    let token = access_token(&[SCOPE_GA4])?;

    // Organic sessions is the metric that matters for SEO; the other reports
    // slice the same organic segment by page, device, and country.
    let organic_filter = json!({
        "filter": {
            "fieldName": "sessionDefaultChannelGroup",
            "stringFilter": {"matchType": "EXACT", "value": "Organic Search"}
        }
    });

    let (dimensions, metrics, with_filter): (Vec<&str>, Vec<&str>, bool) = match report {
        "organic" => (
            vec!["date"],
            vec![
                "sessions",
                "totalUsers",
                "screenPageViews",
                "engagementRate",
            ],
            true,
        ),
        "top-pages" => (
            vec!["pagePath"],
            vec!["sessions", "screenPageViews", "engagementRate"],
            true,
        ),
        "devices" => (vec!["deviceCategory"], vec!["sessions", "totalUsers"], true),
        "countries" => (vec!["country"], vec!["sessions", "totalUsers"], true),
        other => {
            return err(format!(
                "unknown report {other:?}; expected organic, top-pages, devices, or countries"
            ))
        }
    };

    let mut body = json!({
        "dateRanges": [{"startDate": days_ago(days), "endDate": today_utc()}],
        "dimensions": dimensions.iter().map(|d| json!({"name": d})).collect::<Vec<_>>(),
        "metrics": metrics.iter().map(|m| json!({"name": m})).collect::<Vec<_>>(),
        "limit": limit,
    });
    if with_filter {
        body["dimensionFilter"] = organic_filter;
    }

    let url = format!("{GA4_API}/properties/{property_id}:runReport");
    let raw = json_call("POST", &url, Some(&body), &bearer_opts(&token, 60))?;

    let rows: Vec<Value> = raw["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let mut obj = serde_json::Map::new();
            for (i, d) in dimensions.iter().enumerate() {
                obj.insert(d.to_string(), r["dimensionValues"][i]["value"].clone());
            }
            for (i, m) in metrics.iter().enumerate() {
                obj.insert(m.to_string(), r["metricValues"][i]["value"].clone());
            }
            Value::Object(obj)
        })
        .collect();

    let result = json!({
        "property": format!("properties/{property_id}"),
        "report": report,
        "start_date": days_ago(days),
        "end_date": today_utc(),
        "row_count": rows.len(),
        "rows": rows,
    });

    if json {
        print_json(&result)?;
    } else {
        println!(
            "GA4 properties/{property_id} — {report} ({} rows)",
            rows.len()
        );
        for r in rows.iter().take(30) {
            let label = dimensions
                .iter()
                .map(|d| r[*d].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join(" | ");
            let vals = metrics
                .iter()
                .map(|m| format!("{}={}", m, r[*m].as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join("  ");
            println!("  {:<40} {vals}", truncate(&label, 40));
        }
    }
    OK
}

// -------------------------------------------------------------------- NLP

pub fn nlp_analyze(
    url: Option<&str>,
    text: Option<&str>,
    file: Option<&str>,
    features: &[String],
    json: bool,
) -> CmdResult<ExitCode> {
    let key = require_key(&load_config())?;

    let content = match (text, file, url) {
        (Some(t), _, _) => t.to_string(),
        (None, Some(f), _) => {
            let raw = std::fs::read_to_string(f)?;
            if raw.trim_start().starts_with('<') {
                crate::html::visible_text(&raw)
            } else {
                raw
            }
        }
        (None, None, Some(u)) => {
            let rec = crate::cmd::core::fetch_record(u, 30, true, None);
            match rec.error {
                Some(e) => return err(e),
                None => crate::html::visible_text(&rec.content.unwrap_or_default()),
            }
        }
        (None, None, None) => return err("pass --url, --text, or --file"),
    };
    if content.trim().is_empty() {
        return err("no text to analyse");
    }

    let wanted: Vec<String> = if features.is_empty() {
        vec!["entities".into(), "sentiment".into(), "categories".into()]
    } else {
        features.iter().map(|f| f.trim().to_lowercase()).collect()
    };

    // The API caps documents at 1,000,000 bytes; trim well below that so a
    // long page never wastes a round trip.
    let trimmed: String = content.chars().take(200_000).collect();
    let body = json!({
        "document": {"type": "PLAIN_TEXT", "content": trimmed},
        "features": {
            "extractEntities": wanted.iter().any(|f| f == "entities"),
            "extractDocumentSentiment": wanted.iter().any(|f| f == "sentiment"),
            "classifyText": wanted.iter().any(|f| f == "categories"),
            "extractSyntax": wanted.iter().any(|f| f == "syntax"),
        },
        "encodingType": "UTF8",
    });

    let raw = json_call("POST", NLP_ENDPOINT, Some(&body), &key_opts(&key, 45))?;

    let mut entities: Vec<Value> = raw["entities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            json!({
                "name": e["name"],
                "type": e["type"],
                "salience": e["salience"],
                "mentions": e["mentions"].as_array().map(|m| m.len()).unwrap_or(0),
                "wikipedia_url": e["metadata"]["wikipedia_url"],
                "mid": e["metadata"]["mid"],
            })
        })
        .collect();
    entities.sort_by(|a, b| {
        b["salience"]
            .as_f64()
            .partial_cmp(&a["salience"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let result = json!({
        "source": url.or(file).unwrap_or("(inline text)"),
        "language": raw["language"],
        "entity_count": entities.len(),
        "entities": entities,
        "document_sentiment": raw["documentSentiment"],
        "categories": raw["categories"],
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Language: {}", result["language"]);
        println!("Entities ({}):", entities.len());
        for e in entities.iter().take(25) {
            println!(
                "  {:<35} {:<14} salience={:.4}",
                truncate(e["name"].as_str().unwrap_or(""), 35),
                e["type"].as_str().unwrap_or(""),
                e["salience"].as_f64().unwrap_or(0.0)
            );
        }
        if let Some(cats) = result["categories"].as_array() {
            println!("Categories:");
            for c in cats {
                println!(
                    "  {} ({:.2})",
                    c["name"],
                    c["confidence"].as_f64().unwrap_or(0.0)
                );
            }
        }
    }
    OK
}

// ---------------------------------------------------------------- YouTube

pub fn youtube(action: YoutubeAction) -> CmdResult<ExitCode> {
    let key = require_key(&load_config())?;
    match action {
        YoutubeAction::Search { query, limit, json } => {
            let endpoint = format!(
                "{YOUTUBE_API}/search?part=snippet&type=video&maxResults={}&q={}",
                limit.min(50),
                http::enc(&query)
            );
            let raw = json_call("GET", &endpoint, None, &key_opts(&key, 30))?;
            let items: Vec<Value> = raw["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|i| {
                    let vid = i["id"]["videoId"].as_str().unwrap_or("").to_string();
                    json!({
                        "video_id": vid,
                        "title": i["snippet"]["title"],
                        "channel": i["snippet"]["channelTitle"],
                        "published_at": i["snippet"]["publishedAt"],
                        "description": i["snippet"]["description"],
                        "url": format!("https://www.youtube.com/watch?v={vid}"),
                    })
                })
                .collect();
            let result = json!({"query": query, "count": items.len(), "items": items});
            if json {
                print_json(&result)?;
            } else {
                for i in &items {
                    println!(
                        "  {:<60} {}",
                        truncate(i["title"].as_str().unwrap_or(""), 60),
                        i["url"].as_str().unwrap_or("")
                    );
                }
            }
            OK
        }
        YoutubeAction::Video { video_id, json } => {
            let endpoint = format!(
                "{YOUTUBE_API}/videos?part=snippet,statistics,contentDetails&id={}",
                http::enc(&video_id)
            );
            let raw = json_call("GET", &endpoint, None, &key_opts(&key, 30))?;
            let item = raw["items"][0].clone();
            if item.is_null() {
                return err(format!("no video found for id {video_id:?}"));
            }
            let result = json!({
                "video_id": video_id,
                "url": format!("https://www.youtube.com/watch?v={video_id}"),
                "title": item["snippet"]["title"],
                "channel": item["snippet"]["channelTitle"],
                "published_at": item["snippet"]["publishedAt"],
                "tags": item["snippet"]["tags"],
                "duration": item["contentDetails"]["duration"],
                "statistics": item["statistics"],
            });
            if json {
                print_json(&result)?;
            } else {
                println!("{}", result["title"].as_str().unwrap_or(""));
                println!("  channel:   {}", result["channel"].as_str().unwrap_or(""));
                println!("  views:     {}", result["statistics"]["viewCount"]);
                println!("  duration:  {}", result["duration"].as_str().unwrap_or(""));
            }
            OK
        }
    }
}

// -------------------------------------------------------- keyword planner

pub fn keyword_planner(action: KeywordAction) -> CmdResult<ExitCode> {
    let cfg = load_config();
    let (Some(dev_token), Some(customer_id)) =
        (cfg.ads_developer_token.clone(), cfg.ads_customer_id.clone())
    else {
        return err(
            "Keyword Planner needs Google Ads API access. Set GOOGLE_ADS_DEVELOPER_TOKEN and \
             GOOGLE_ADS_CUSTOMER_ID (digits only), and point GOOGLE_APPLICATION_CREDENTIALS at a \
             service account with Ads access. See `seogeo google-auth --setup`.",
        );
    };
    let token = access_token(&["https://www.googleapis.com/auth/adwords"])?;
    let customer_id = customer_id.replace('-', "");
    let opts = bearer_opts(&token, 60)
        .header("developer-token", dev_token)
        .header("login-customer-id", customer_id.clone());

    match action {
        KeywordAction::Ideas {
            seeds,
            url,
            country,
            json,
        } => {
            if seeds.is_empty() && url.is_none() {
                return err("pass seed keywords or --url");
            }
            let mut body = json!({
                "language": "languageConstants/1000",
                "geoTargetConstants": [geo_target(&country)],
                "includeAdultKeywords": false,
            });
            if !seeds.is_empty() && url.is_some() {
                body["keywordAndUrlSeed"] = json!({"keywords": seeds, "url": url});
            } else if !seeds.is_empty() {
                body["keywordSeed"] = json!({"keywords": seeds});
            } else {
                body["urlSeed"] = json!({"url": url});
            }
            let endpoint = format!("{ADS_API}/customers/{customer_id}:generateKeywordIdeas");
            let raw = json_call("POST", &endpoint, Some(&body), &opts)?;
            let ideas: Vec<Value> = raw["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|r| {
                    json!({
                        "keyword": r["text"],
                        "avg_monthly_searches": r["keywordIdeaMetrics"]["avgMonthlySearches"],
                        "competition": r["keywordIdeaMetrics"]["competition"],
                        "competition_index": r["keywordIdeaMetrics"]["competitionIndex"],
                        "low_top_of_page_bid_micros": r["keywordIdeaMetrics"]["lowTopOfPageBidMicros"],
                        "high_top_of_page_bid_micros": r["keywordIdeaMetrics"]["highTopOfPageBidMicros"],
                    })
                })
                .collect();
            let result = json!({"seeds": seeds, "url": url, "country": country, "count": ideas.len(), "ideas": ideas});
            if json {
                print_json(&result)?;
            } else {
                for i in ideas.iter().take(50) {
                    println!(
                        "  {:<45} {:<10} {}",
                        truncate(i["keyword"].as_str().unwrap_or(""), 45),
                        i["avg_monthly_searches"],
                        i["competition"].as_str().unwrap_or("")
                    );
                }
            }
            OK
        }
        KeywordAction::Volume {
            keywords,
            country,
            json,
        } => {
            if keywords.is_empty() {
                return err("pass at least one keyword");
            }
            let body = json!({
                "language": "languageConstants/1000",
                "geoTargetConstants": [geo_target(&country)],
                "keywords": keywords,
                "historicalMetricsOptions": {"includeAverageCpc": true},
            });
            let endpoint =
                format!("{ADS_API}/customers/{customer_id}:generateKeywordHistoricalMetrics");
            let raw = json_call("POST", &endpoint, Some(&body), &opts)?;
            if json {
                print_json(&raw)?;
            } else {
                for r in raw["results"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "  {:<45} {}",
                        truncate(r["text"].as_str().unwrap_or(""), 45),
                        r["keywordMetrics"]["avgMonthlySearches"]
                    );
                }
            }
            OK
        }
    }
}

/// Ads geo target constants for the countries an SEO audit normally covers.
fn geo_target(country: &str) -> String {
    let id = match country.to_uppercase().as_str() {
        "US" => 2840,
        "GB" | "UK" => 2826,
        "CA" => 2124,
        "AU" => 2036,
        "DE" => 2276,
        "FR" => 2250,
        "ES" => 2724,
        "IT" => 2380,
        "NL" => 2528,
        "JP" => 2392,
        "IN" => 2356,
        "BR" => 2076,
        "MX" => 2484,
        "CN" => 2156,
        _ => 2840,
    };
    format!("geoTargetConstants/{id}")
}

// ----------------------------------------------------------------- report

/// Render an audit JSON payload into a self-contained HTML report, and
/// optionally print it to PDF with headless Chrome.
pub fn report(
    report_type: &str,
    data_path: &str,
    domain: &str,
    format: &str,
    output_dir: Option<&str>,
) -> CmdResult<ExitCode> {
    let raw = std::fs::read_to_string(data_path)
        .map_err(|e| Error(format!("could not read {data_path}: {e}")))?;
    let data: Value = serde_json::from_str(&raw)?;

    let dir = std::path::Path::new(output_dir.unwrap_or("."));
    std::fs::create_dir_all(dir)?;
    let stem = format!("{}-{report_type}-report", domain.replace(['/', ':'], "_"));
    let html_path = dir.join(format!("{stem}.html"));

    let html = render_report_html(report_type, domain, &data);
    std::fs::write(&html_path, html)?;

    if format == "pdf" {
        let pdf_path = dir.join(format!("{stem}.pdf"));
        crate::chrome::print_pdf(&html_path, &pdf_path, 8000)?;
        println!("{}", pdf_path.display());
        eprintln!("Wrote {} and {}", html_path.display(), pdf_path.display());
    } else {
        println!("{}", html_path.display());
        eprintln!("Wrote {}", html_path.display());
    }
    OK
}

fn render_report_html(report_type: &str, domain: &str, data: &Value) -> String {
    use crate::cmd::drift::esc;
    let mut sections = String::new();
    render_value(data, 2, &mut sections);

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title} — {domain_esc}</title>
<style>
:root {{ --navy:#1e3a5f; --gold:#b8860b; --green:#2d6a4f; --amber:#d4740e;
        --red:#c53030; --cream:#faf9f7; --line:#e3e7eb; --fg:#16191d; --muted:#5b6570; --bg:#fff; }}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg:#14171a; --fg:#e8eaed; --muted:#9aa4af; --line:#2a2f35; --cream:#1c2025; }}
}}
@page {{ size:A4; margin:18mm 16mm; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; padding:2rem 1.25rem; background:var(--bg); color:var(--fg);
        font:14px/1.65 "Iowan Old Style",Georgia,"Times New Roman",serif; }}
.wrap {{ max-width:940px; margin:0 auto; }}
header {{ border-bottom:3px solid var(--navy); padding-bottom:1rem; margin-bottom:2rem; }}
h1 {{ color:var(--navy); font-size:1.7rem; margin:0 0 .3rem; }}
.meta {{ color:var(--muted); font-size:.85rem; }}
h2 {{ color:var(--navy); font-size:1.1rem; border-bottom:1px solid var(--line);
      padding-bottom:.3rem; margin-top:2rem; }}
h3 {{ color:var(--gold); font-size:.95rem; margin-top:1.4rem; }}
.tablewrap {{ overflow-x:auto; }}
table {{ border-collapse:collapse; width:100%; margin:.6rem 0; font-size:.86rem; }}
th,td {{ text-align:left; padding:.45rem .6rem; border-bottom:1px solid var(--line);
         vertical-align:top; }}
th {{ background:var(--cream); font-weight:600; width:32%; }}
code,pre {{ font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.82em; }}
pre {{ background:var(--cream); padding:.7rem .9rem; border-radius:8px; overflow-x:auto; }}
ul {{ padding-left:1.2rem; }}
.val-num {{ font-variant-numeric:tabular-nums; }}
</style></head><body><div class="wrap">
<header>
  <h1>{title}</h1>
  <div class="meta">{domain_esc} &middot; generated {date} by seogeo</div>
</header>
{sections}
</div></body></html>"#,
        title = esc(&report_type_title(report_type)),
        domain_esc = esc(domain),
        date = crate::output::today_utc(),
    )
}

fn report_type_title(t: &str) -> String {
    match t {
        "full" => "Full SEO audit".into(),
        "cwv-audit" => "Core Web Vitals audit".into(),
        "geo" => "GEO visibility report".into(),
        "technical" => "Technical SEO report".into(),
        "content" => "Content quality report".into(),
        other => format!("{} report", other.replace('-', " ")),
    }
}

/// Turn arbitrary audit JSON into readable HTML: objects become definition
/// tables, arrays of objects become rows, scalars become values.
fn render_value(value: &Value, depth: usize, out: &mut String) {
    use crate::cmd::drift::esc;
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                let heading = humanize_key(key);
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        let tag = if depth <= 2 { "h2" } else { "h3" };
                        out.push_str(&format!("<{tag}>{}</{tag}>", esc(&heading)));
                        render_value(v, depth + 1, out);
                    }
                    scalar => {
                        out.push_str(&format!(
                            "<div class=\"tablewrap\"><table><tr><th>{}</th><td>{}</td></tr></table></div>",
                            esc(&heading),
                            esc(&scalar_text(scalar))
                        ));
                    }
                }
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("<p class=\"meta\">(none)</p>");
                return;
            }
            if items.iter().all(|i| i.is_object()) {
                let mut columns: Vec<String> = Vec::new();
                for item in items {
                    for k in item.as_object().unwrap().keys() {
                        if !columns.contains(k) {
                            columns.push(k.clone());
                        }
                    }
                }
                out.push_str("<div class=\"tablewrap\"><table><thead><tr>");
                for c in &columns {
                    out.push_str(&format!("<th>{}</th>", esc(&humanize_key(c))));
                }
                out.push_str("</tr></thead><tbody>");
                for item in items {
                    out.push_str("<tr>");
                    for c in &columns {
                        out.push_str(&format!("<td>{}</td>", esc(&scalar_text(&item[c]))));
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table></div>");
            } else {
                out.push_str("<ul>");
                for item in items {
                    out.push_str(&format!("<li>{}</li>", esc(&scalar_text(item))));
                }
                out.push_str("</ul>");
            }
        }
        scalar => out.push_str(&format!("<p>{}</p>", esc(&scalar_text(scalar)))),
    }
}

fn humanize_key(key: &str) -> String {
    let spaced = key.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Value::Number(n) => n.to_string(),
        other => truncate(&other.to_string(), 400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_api_keys() {
        let s = "failed: https://x/api?key=AIzaSyDUMMYKEY123&url=y";
        let out = redact(s);
        assert!(!out.contains("AIzaSy"));
        assert!(out.contains("key=REDACTED"));
    }

    #[test]
    fn geo_targets_map_known_countries() {
        assert_eq!(geo_target("GB"), "geoTargetConstants/2826");
        assert_eq!(geo_target("uk"), "geoTargetConstants/2826");
        assert_eq!(geo_target("zz"), "geoTargetConstants/2840");
    }

    #[test]
    fn report_html_renders_nested_json() {
        let data =
            json!({"scores": {"performance": 91}, "issues": [{"id": "a", "severity": "high"}]});
        let html = render_report_html("full", "example.com", &data);
        assert!(html.contains("Performance"));
        assert!(html.contains("Severity"));
        assert!(html.contains("example.com"));
    }
}

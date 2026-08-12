//! DataForSEO commands: cost estimation with a persistent budget ledger,
//! response normalisation, and the Merchant (Google Shopping / Amazon) API.
//!
//! DataForSEO bills per request, so every call is priced before it is made
//! and recorded after. The ledger lives beside the other seogeo state.

use std::process::ExitCode;

use base64::Engine;
use serde_json::{json, Value};

use crate::cli::{DfsCostAction, DfsMerchantAction};
use crate::http::{self, RequestOptions};
use crate::output::{err, money, now_utc, print_json, today_utc, truncate, CmdResult, Error};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);
const API_BASE: &str = "https://api.dataforseo.com/v3";

/// Published list prices in USD per request. Live pricing moves; treat these
/// as planning estimates and reconcile with `dataforseo-costs log`.
const PRICES: &[(&str, f64, &str)] = &[
    ("serp_organic_live_advanced", 0.002, "SERP: organic results, live advanced"),
    ("serp_organic_live_regular", 0.0006, "SERP: organic results, live regular"),
    ("serp_ai_mode_live_advanced", 0.006, "SERP: AI Mode, live advanced"),
    ("serp_google_ai_summary", 0.006, "SERP: AI Overview extraction"),
    ("keywords_search_volume", 0.05, "Keywords Data: search volume (per 1000 keywords)"),
    ("keywords_for_keywords", 0.05, "Keywords Data: keyword ideas"),
    ("keywords_for_site", 0.05, "Keywords Data: keywords for site"),
    ("dataforseo_labs_keyword_ideas", 0.011, "Labs: keyword ideas"),
    ("dataforseo_labs_ranked_keywords", 0.011, "Labs: ranked keywords"),
    ("dataforseo_labs_competitors_domain", 0.011, "Labs: competitor domains"),
    ("dataforseo_labs_domain_intersection", 0.011, "Labs: keyword gap"),
    ("backlinks_summary", 0.02, "Backlinks: summary"),
    ("backlinks_backlinks", 0.02, "Backlinks: individual links"),
    ("backlinks_referring_domains", 0.02, "Backlinks: referring domains"),
    ("backlinks_anchors", 0.02, "Backlinks: anchor text"),
    ("on_page_task_post", 0.0006, "On-Page: crawl (per page)"),
    ("on_page_instant_pages", 0.00125, "On-Page: instant page audit"),
    ("content_analysis_search", 0.02, "Content Analysis: search"),
    ("business_data_google_my_business", 0.002, "Business Data: GBP info"),
    ("business_data_google_reviews", 0.0015, "Business Data: reviews"),
    ("merchant_google_products_search", 0.003, "Merchant: Google Shopping product search"),
    ("merchant_google_sellers", 0.003, "Merchant: Google Shopping sellers"),
    ("merchant_amazon_products_search", 0.003, "Merchant: Amazon product search"),
    ("merchant_amazon_sellers", 0.003, "Merchant: Amazon sellers"),
    ("domain_analytics_technologies", 0.011, "Domain Analytics: technologies"),
    ("domain_analytics_whois", 0.011, "Domain Analytics: WHOIS"),
];

fn ledger_path() -> std::path::PathBuf {
    crate::paths::data_dir().join("dataforseo-costs.json")
}

fn load_ledger() -> Value {
    std::fs::read_to_string(ledger_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({"budget_usd": null, "entries": []}))
}

fn save_ledger(ledger: &Value) -> CmdResult<()> {
    let path = ledger_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(ledger)?)?;
    Ok(())
}

fn spend_to_date(ledger: &Value) -> f64 {
    money(
        ledger["entries"]
            .as_array()
            .map(|e| e.iter().filter_map(|x| x["cost"].as_f64()).sum::<f64>())
            .unwrap_or(0.0),
    )
}

fn price_of(endpoint: &str) -> Option<(f64, &'static str)> {
    PRICES
        .iter()
        .find(|(k, _, _)| *k == endpoint)
        .map(|(_, p, d)| (*p, *d))
}

pub fn costs(action: DfsCostAction) -> CmdResult<ExitCode> {
    match action {
        DfsCostAction::Check {
            endpoint,
            count,
            json,
        } => {
            let Some((unit, description)) = price_of(&endpoint) else {
                return err(format!(
                    "unknown endpoint {endpoint:?}. Run `seogeo dataforseo-costs list` for the \
                     priced endpoints."
                ));
            };
            let total = money(unit * count as f64);
            let ledger = load_ledger();
            let spent = spend_to_date(&ledger);
            let result = json!({
                "endpoint": endpoint,
                "description": description,
                "unit_cost_usd": unit,
                "count": count,
                "estimated_cost_usd": total,
                "spent_to_date_usd": spent,
                "budget_usd": ledger["budget_usd"],
            });
            if json {
                print_json(&result)?;
            } else {
                println!("{endpoint} — {description}");
                println!("  unit:      ${unit:.5}");
                println!("  count:     {count}");
                println!("  estimated: ${total:.5}");
                println!("  spent:     ${spent:.5}");
            }
            OK
        }
        DfsCostAction::Log {
            endpoint,
            cost,
            json,
        } => {
            let mut ledger = load_ledger();
            let entry = json!({
                "endpoint": endpoint,
                "cost": cost,
                "timestamp": now_utc(),
                "date": today_utc(),
            });
            ledger["entries"]
                .as_array_mut()
                .ok_or_else(|| Error("ledger is corrupt: `entries` is not an array".into()))?
                .push(entry.clone());
            save_ledger(&ledger)?;
            let spent = spend_to_date(&ledger);
            let result = json!({"logged": entry, "spent_to_date_usd": spent, "ledger": ledger_path().display().to_string()});
            if json {
                print_json(&result)?;
            } else {
                println!("Logged ${cost:.5} for {endpoint}. Total spend: ${spent:.5}");
            }
            OK
        }
        DfsCostAction::Summary { json } => {
            let ledger = load_ledger();
            let empty = vec![];
            let entries = ledger["entries"].as_array().unwrap_or(&empty);
            let spent = spend_to_date(&ledger);

            let mut by_endpoint: std::collections::BTreeMap<String, (usize, f64)> =
                Default::default();
            for e in entries {
                let key = e["endpoint"].as_str().unwrap_or("?").to_string();
                let slot = by_endpoint.entry(key).or_insert((0, 0.0));
                slot.0 += 1;
                slot.1 += e["cost"].as_f64().unwrap_or(0.0);
            }

            let result = json!({
                "ledger": ledger_path().display().to_string(),
                "entries": entries.len(),
                "spent_to_date_usd": spent,
                "budget_usd": ledger["budget_usd"],
                "by_endpoint": by_endpoint.iter().map(|(k, (n, c))| {
                    json!({"endpoint": k, "calls": n, "cost_usd": money(*c)})
                }).collect::<Vec<_>>(),
            });
            if json {
                print_json(&result)?;
            } else {
                println!("Ledger: {}", ledger_path().display());
                println!("Entries: {}  Total: ${spent:.5}", entries.len());
                for (k, (n, c)) in &by_endpoint {
                    println!("  {k:<40} {n:>4} calls  ${c:.5}");
                }
            }
            OK
        }
        DfsCostAction::List { json } => {
            let rows: Vec<Value> = PRICES
                .iter()
                .map(|(k, p, d)| json!({"endpoint": k, "unit_cost_usd": p, "description": d}))
                .collect();
            if json {
                print_json(&json!({"count": rows.len(), "endpoints": rows}))?;
            } else {
                for r in &rows {
                    println!(
                        "  {:<40} ${:<9} {}",
                        r["endpoint"].as_str().unwrap_or(""),
                        r["unit_cost_usd"],
                        r["description"].as_str().unwrap_or("")
                    );
                }
            }
            OK
        }
    }
}

// -------------------------------------------------------------- normalize

/// Flatten a raw DataForSEO envelope into the row shape the skills consume.
/// The API nests every payload under `tasks[].result[].items[]`, and each
/// module names its fields differently; this collapses both.
pub fn normalize(file: &str, module: &str, json: bool) -> CmdResult<ExitCode> {
    let raw = std::fs::read_to_string(file)
        .map_err(|e| Error(format!("could not read {file}: {e}")))?;
    let data: Value = serde_json::from_str(&raw)?;

    let status = data["status_code"].as_i64();
    let mut items: Vec<Value> = Vec::new();
    let mut cost = 0.0;

    if let Some(tasks) = data["tasks"].as_array() {
        for task in tasks {
            cost += task["cost"].as_f64().unwrap_or(0.0);
            if let Some(results) = task["result"].as_array() {
                for result in results {
                    match result["items"].as_array() {
                        Some(list) => items.extend(list.iter().cloned()),
                        None => items.push(result.clone()),
                    }
                }
            }
        }
    } else if let Some(list) = data.as_array() {
        items = list.clone();
    } else {
        items.push(data.clone());
    }

    let rows: Vec<Value> = items.iter().map(|i| normalize_item(i, module)).collect();
    let result = json!({
        "source_file": file,
        "module": module,
        "status_code": status,
        "cost_usd": cost,
        "row_count": rows.len(),
        "rows": rows,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("{file} -> {module}: {} row(s), cost ${cost:.5}", rows.len());
        for r in rows.iter().take(20) {
            println!("  {}", truncate(&r.to_string(), 160));
        }
    }
    OK
}

fn normalize_item(item: &Value, module: &str) -> Value {
    let take = |keys: &[&str]| -> Value {
        for k in keys {
            if !item[*k].is_null() {
                return item[*k].clone();
            }
        }
        Value::Null
    };
    match module {
        "serp" => json!({
            "type": item["type"],
            "rank": take(&["rank_absolute", "rank_group"]),
            "title": item["title"],
            "url": take(&["url", "link"]),
            "domain": item["domain"],
            "description": take(&["description", "snippet"]),
        }),
        "keywords" => json!({
            "keyword": take(&["keyword", "se_keyword"]),
            "search_volume": take(&["search_volume", "keyword_info.search_volume"]),
            "competition": item["competition"],
            "cpc": item["cpc"],
            "difficulty": take(&["keyword_difficulty", "keyword_properties"]),
        }),
        "backlinks" => json!({
            "url_from": item["url_from"],
            "url_to": item["url_to"],
            "domain_from": item["domain_from"],
            "anchor": item["anchor"],
            "dofollow": item["dofollow"],
            "rank": item["rank"],
            "first_seen": item["first_seen"],
        }),
        "merchant" => json!({
            "title": item["title"],
            "seller": take(&["seller", "shop_name", "domain"]),
            "price": take(&["price", "current_price"]),
            "currency": item["currency"],
            "rating": item["rating"],
            "url": take(&["url", "product_url", "link"]),
            "product_id": take(&["product_id", "data_asin", "asin"]),
        }),
        "onpage" => json!({
            "url": item["url"],
            "status_code": item["status_code"],
            "title": take(&["meta.title", "title"]),
            "issues": item["checks"],
        }),
        _ => item.clone(),
    }
}

// --------------------------------------------------------------- merchant

fn credentials() -> CmdResult<String> {
    let login = std::env::var("DATAFORSEO_LOGIN").ok();
    let password = std::env::var("DATAFORSEO_PASSWORD").ok();
    match (login, password) {
        (Some(l), Some(p)) if !l.is_empty() && !p.is_empty() => Ok(
            base64::engine::general_purpose::STANDARD.encode(format!("{l}:{p}")),
        ),
        _ => err(
            "DataForSEO credentials missing. Set DATAFORSEO_LOGIN and DATAFORSEO_PASSWORD \
             (sign up at https://dataforseo.com).",
        ),
    }
}

fn dfs_post(path: &str, body: &Value) -> CmdResult<Value> {
    let auth = credentials()?;
    let opts = RequestOptions::with_timeout(90).header("Authorization", format!("Basic {auth}"));
    let resp = http::post_json(&format!("{API_BASE}{path}"), body, &opts)?;
    let text = resp.text();
    if !(200..300).contains(&resp.status) {
        return err(format!("DataForSEO HTTP {}: {}", resp.status, truncate(&text, 400)));
    }
    let v: Value = serde_json::from_str(&text)?;
    if v["status_code"].as_i64().unwrap_or(0) != 20000 {
        return err(format!(
            "DataForSEO error {}: {}",
            v["status_code"],
            v["status_message"].as_str().unwrap_or("")
        ));
    }
    Ok(v)
}

fn merchant_path(marketplace: &str, kind: &str) -> CmdResult<String> {
    match (marketplace, kind) {
        ("google", "search") => Ok("/merchant/google/products/live/advanced".into()),
        ("google", "sellers") => Ok("/merchant/google/sellers/live/advanced".into()),
        ("amazon", "search") => Ok("/merchant/amazon/products/live/advanced".into()),
        ("amazon", "sellers") => Ok("/merchant/amazon/sellers/live/advanced".into()),
        (m, _) => err(format!("unknown marketplace {m:?}; expected google or amazon")),
    }
}

pub fn merchant(action: DfsMerchantAction) -> CmdResult<ExitCode> {
    match action {
        DfsMerchantAction::Search {
            keyword,
            marketplace,
            location,
            json,
        } => {
            let path = merchant_path(&marketplace, "search")?;
            let body = json!([{
                "keyword": keyword,
                "location_name": location,
                "language_code": "en",
            }]);
            let raw = dfs_post(&path, &body)?;
            emit_merchant(&raw, &keyword, &marketplace, json)
        }
        DfsMerchantAction::Compare {
            keyword,
            location,
            json,
        } => {
            // Same query on both marketplaces is what makes the comparison
            // meaningful; anything else is comparing different demand.
            let mut per_market = serde_json::Map::new();
            for market in ["google", "amazon"] {
                let path = merchant_path(market, "search")?;
                let body = json!([{
                    "keyword": keyword,
                    "location_name": location,
                    "language_code": "en",
                }]);
                match dfs_post(&path, &body) {
                    Ok(raw) => {
                        let rows = merchant_rows(&raw);
                        let prices: Vec<f64> =
                            rows.iter().filter_map(|r| r["price"].as_f64()).collect();
                        per_market.insert(
                            market.to_string(),
                            json!({
                                "results": rows.len(),
                                "min_price": prices.iter().cloned().fold(f64::INFINITY, f64::min),
                                "max_price": prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                                "median_price": median(&prices),
                                "items": rows,
                            }),
                        );
                    }
                    Err(e) => {
                        per_market.insert(market.to_string(), json!({"error": e.to_string()}));
                    }
                }
            }
            let result = json!({"keyword": keyword, "location": location, "marketplaces": per_market});
            if json {
                print_json(&result)?;
            } else {
                for (m, data) in result["marketplaces"].as_object().unwrap() {
                    println!(
                        "{m}: {} results, median price {}",
                        data["results"], data["median_price"]
                    );
                }
            }
            OK
        }
        DfsMerchantAction::Sellers {
            product_id,
            marketplace,
            location,
            json,
        } => {
            let path = merchant_path(&marketplace, "sellers")?;
            let key = if marketplace == "amazon" { "asin" } else { "product_id" };
            let body = json!([{
                key: product_id,
                "location_name": location,
                "language_code": "en",
            }]);
            let raw = dfs_post(&path, &body)?;
            emit_merchant(&raw, &product_id, &marketplace, json)
        }
    }
}

fn merchant_rows(raw: &Value) -> Vec<Value> {
    let mut rows = Vec::new();
    if let Some(tasks) = raw["tasks"].as_array() {
        for task in tasks {
            if let Some(results) = task["result"].as_array() {
                for result in results {
                    if let Some(items) = result["items"].as_array() {
                        for item in items {
                            rows.push(normalize_item(item, "merchant"));
                        }
                    }
                }
            }
        }
    }
    rows
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

fn emit_merchant(raw: &Value, query: &str, marketplace: &str, json: bool) -> CmdResult<ExitCode> {
    let rows = merchant_rows(raw);
    let cost: f64 = raw["tasks"]
        .as_array()
        .map(|t| t.iter().filter_map(|x| x["cost"].as_f64()).sum())
        .unwrap_or(0.0);
    let result = json!({
        "query": query,
        "marketplace": marketplace,
        "count": rows.len(),
        "cost_usd": cost,
        "items": rows,
    });
    if json {
        print_json(&result)?;
    } else {
        println!("{marketplace}: {} result(s) for {query:?} (${cost:.5})", rows.len());
        for r in rows.iter().take(25) {
            println!(
                "  {:<50} {} {}",
                truncate(r["title"].as_str().unwrap_or(""), 50),
                r["price"],
                r["seller"].as_str().unwrap_or("")
            );
        }
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_unique_and_positive() {
        let mut keys: Vec<&str> = PRICES.iter().map(|(k, _, _)| *k).collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate endpoint in the price table");
        assert!(PRICES.iter().all(|(_, p, _)| *p > 0.0));
    }

    #[test]
    fn normalizes_serp_envelope() {
        let raw = json!({
            "status_code": 20000,
            "tasks": [{"cost": 0.002, "result": [{"items": [
                {"type": "organic", "rank_absolute": 1, "title": "A", "url": "https://a.test"}
            ]}]}]
        });
        let mut items = Vec::new();
        for task in raw["tasks"].as_array().unwrap() {
            for result in task["result"].as_array().unwrap() {
                for item in result["items"].as_array().unwrap() {
                    items.push(normalize_item(item, "serp"));
                }
            }
        }
        assert_eq!(items[0]["rank"], 1);
        assert_eq!(items[0]["url"], "https://a.test");
    }

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&[1.0, 3.0]), Some(2.0));
        assert_eq!(median(&[1.0, 3.0, 10.0]), Some(3.0));
        assert_eq!(median(&[]), None);
    }
}

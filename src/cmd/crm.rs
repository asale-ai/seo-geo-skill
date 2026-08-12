//! CRM-lite for GEO/SEO client work: prospects move through a fixed
//! pipeline, audits and notes accumulate against a domain, and the compare
//! command turns two stored audits into a month-over-month delta.
//!
//! State is a single JSON file so it is trivially inspectable, diffable, and
//! backed up by whatever the user already uses.

use std::process::ExitCode;

use serde_json::{json, Value};

use crate::cli::CrmAction;
use crate::output::{err, money, now_utc, print_json, today_utc, truncate, CmdResult, Error};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

pub const STAGES: &[&str] = &["lead", "qualified", "proposal_sent", "won", "lost"];

fn store_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SEOGEO_CRM_FILE") {
        return std::path::PathBuf::from(p);
    }
    crate::paths::data_dir().join("prospects.json")
}

fn load() -> Value {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({"prospects": []}))
}

fn save(store: &Value) -> CmdResult<()> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(store)?)?;
    Ok(())
}

fn normalize_domain(input: &str) -> String {
    input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn find_index(store: &Value, domain: &str) -> Option<usize> {
    store["prospects"]
        .as_array()?
        .iter()
        .position(|p| p["domain"].as_str() == Some(domain))
}

fn check_stage(stage: &str) -> CmdResult<String> {
    let s = stage.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    if STAGES.contains(&s.as_str()) {
        Ok(s)
    } else {
        err(format!(
            "unknown stage {stage:?}; expected one of {}",
            STAGES.join(", ")
        ))
    }
}

pub fn run(action: CrmAction) -> CmdResult<ExitCode> {
    match action {
        CrmAction::Add {
            domain,
            name,
            email,
            stage,
            value,
            json,
        } => {
            let domain = normalize_domain(&domain);
            let stage = check_stage(&stage)?;
            let mut store = load();
            if find_index(&store, &domain).is_some() {
                return err(format!("{domain} already exists — use `seogeo crm update`"));
            }
            let record = json!({
                "domain": domain,
                "name": name.unwrap_or_else(|| domain.clone()),
                "email": email,
                "stage": stage,
                "deal_value": value,
                "created_at": now_utc(),
                "updated_at": now_utc(),
                "notes": [],
                "audits": [],
            });
            store["prospects"]
                .as_array_mut()
                .ok_or_else(|| Error("store is corrupt: `prospects` is not an array".into()))?
                .push(record.clone());
            save(&store)?;
            if json {
                print_json(&record)?;
            } else {
                println!("Added {domain} at stage {}", record["stage"]);
            }
            OK
        }

        CrmAction::List { stage, json } => {
            let store = load();
            let empty = vec![];
            let all = store["prospects"].as_array().unwrap_or(&empty);
            let filtered: Vec<&Value> = match &stage {
                Some(s) => {
                    let s = check_stage(s)?;
                    all.iter().filter(|p| p["stage"].as_str() == Some(&s)).collect()
                }
                None => all.iter().collect(),
            };
            let result = json!({
                "store": store_path().display().to_string(),
                "count": filtered.len(),
                "prospects": filtered,
            });
            if json {
                print_json(&result)?;
            } else {
                println!("{} prospect(s)", filtered.len());
                for p in &filtered {
                    println!(
                        "  {:<32} {:<14} {:>10}  audits={}",
                        truncate(p["domain"].as_str().unwrap_or(""), 32),
                        p["stage"].as_str().unwrap_or(""),
                        p["deal_value"]
                            .as_f64()
                            .map(|v| format!("${v:.0}"))
                            .unwrap_or_else(|| "-".into()),
                        p["audits"].as_array().map(|a| a.len()).unwrap_or(0)
                    );
                }
            }
            OK
        }

        CrmAction::Show { domain, json } => {
            let domain = normalize_domain(&domain);
            let store = load();
            let idx = find_index(&store, &domain)
                .ok_or_else(|| Error(format!("{domain} not found")))?;
            let record = &store["prospects"][idx];
            if json {
                print_json(record)?;
            } else {
                println!("{}", record["name"].as_str().unwrap_or(""));
                println!("  domain:  {domain}");
                println!("  stage:   {}", record["stage"].as_str().unwrap_or(""));
                println!("  value:   {}", record["deal_value"]);
                println!("  email:   {}", record["email"]);
                println!("  created: {}", record["created_at"].as_str().unwrap_or(""));
                if let Some(audits) = record["audits"].as_array() {
                    println!("  audits:");
                    for a in audits {
                        println!(
                            "    {}  score={}  {}",
                            a["date"].as_str().unwrap_or(""),
                            a["score"],
                            a["report"].as_str().unwrap_or("")
                        );
                    }
                }
                if let Some(notes) = record["notes"].as_array() {
                    println!("  notes:");
                    for n in notes {
                        println!(
                            "    {} — {}",
                            n["date"].as_str().unwrap_or(""),
                            n["text"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            OK
        }

        CrmAction::Update {
            domain,
            stage,
            value,
            json,
        } => {
            let domain = normalize_domain(&domain);
            let mut store = load();
            let idx = find_index(&store, &domain)
                .ok_or_else(|| Error(format!("{domain} not found")))?;
            if let Some(s) = stage {
                store["prospects"][idx]["stage"] = json!(check_stage(&s)?);
            }
            if let Some(v) = value {
                store["prospects"][idx]["deal_value"] = json!(v);
            }
            store["prospects"][idx]["updated_at"] = json!(now_utc());
            let record = store["prospects"][idx].clone();
            save(&store)?;
            if json {
                print_json(&record)?;
            } else {
                println!(
                    "{domain}: stage={} value={}",
                    record["stage"].as_str().unwrap_or(""),
                    record["deal_value"]
                        .as_f64()
                        .map(|v| format!("${v:.0}"))
                        .unwrap_or_else(|| "-".into())
                );
            }
            OK
        }

        CrmAction::Note { domain, text, json } => {
            let domain = normalize_domain(&domain);
            let mut store = load();
            let idx = find_index(&store, &domain)
                .ok_or_else(|| Error(format!("{domain} not found")))?;
            let note = json!({"date": today_utc(), "timestamp": now_utc(), "text": text});
            store["prospects"][idx]["notes"]
                .as_array_mut()
                .ok_or_else(|| Error("prospect record has no notes array".into()))?
                .push(note.clone());
            store["prospects"][idx]["updated_at"] = json!(now_utc());
            save(&store)?;
            if json {
                print_json(&note)?;
            } else {
                println!("Note added to {domain}");
            }
            OK
        }

        CrmAction::Audit {
            domain,
            score,
            report,
            json,
        } => {
            let domain = normalize_domain(&domain);
            let mut store = load();
            let idx = find_index(&store, &domain)
                .ok_or_else(|| Error(format!("{domain} not found")))?;
            let audit = json!({
                "date": today_utc(),
                "timestamp": now_utc(),
                "score": score,
                "report": report,
            });
            store["prospects"][idx]["audits"]
                .as_array_mut()
                .ok_or_else(|| Error("prospect record has no audits array".into()))?
                .push(audit.clone());
            store["prospects"][idx]["updated_at"] = json!(now_utc());
            save(&store)?;
            if json {
                print_json(&audit)?;
            } else {
                println!("Recorded audit for {domain}: score {score}");
            }
            OK
        }

        CrmAction::Pipeline { json } => {
            let store = load();
            let empty = vec![];
            let all = store["prospects"].as_array().unwrap_or(&empty);
            let mut stages = Vec::new();
            for stage in STAGES {
                let members: Vec<&Value> = all
                    .iter()
                    .filter(|p| p["stage"].as_str() == Some(*stage))
                    .collect();
                let value = money(
                    members
                        .iter()
                        .filter_map(|p| p["deal_value"].as_f64())
                        .sum::<f64>(),
                );
                stages.push(json!({
                    "stage": stage,
                    "count": members.len(),
                    "value_usd": value,
                    "domains": members.iter().filter_map(|p| p["domain"].as_str()).collect::<Vec<_>>(),
                }));
            }
            let open_value = money(
                stages
                    .iter()
                    .filter(|s| !matches!(s["stage"].as_str(), Some("won") | Some("lost")))
                    .filter_map(|s| s["value_usd"].as_f64())
                    .sum::<f64>(),
            );
            let won_value = money(
                stages
                    .iter()
                    .filter(|s| s["stage"] == "won")
                    .filter_map(|s| s["value_usd"].as_f64())
                    .sum::<f64>(),
            );

            let result = json!({
                "total_prospects": all.len(),
                "open_pipeline_usd": open_value,
                "won_usd": won_value,
                "stages": stages,
            });
            if json {
                print_json(&result)?;
            } else {
                println!("Pipeline ({} prospects)", all.len());
                for s in &stages {
                    println!(
                        "  {:<16} {:>3}  ${:.0}",
                        s["stage"].as_str().unwrap_or(""),
                        s["count"],
                        s["value_usd"].as_f64().unwrap_or(0.0)
                    );
                }
                println!("  {:<16} {:>3}  ${open_value:.0}", "OPEN", "");
                println!("  {:<16} {:>3}  ${won_value:.0}", "WON", "");
            }
            OK
        }

        CrmAction::Compare { domain, json } => {
            let domain = normalize_domain(&domain);
            let store = load();
            let idx = find_index(&store, &domain)
                .ok_or_else(|| Error(format!("{domain} not found")))?;
            let empty = vec![];
            let audits = store["prospects"][idx]["audits"]
                .as_array()
                .unwrap_or(&empty);
            if audits.len() < 2 {
                return err(format!(
                    "{domain} has {} audit(s); at least two are needed for a delta. Record one \
                     with `seogeo crm audit {domain} --score <n>`.",
                    audits.len()
                ));
            }
            let previous = &audits[audits.len() - 2];
            let current = &audits[audits.len() - 1];
            let (p, c) = (
                previous["score"].as_f64().unwrap_or(0.0),
                current["score"].as_f64().unwrap_or(0.0),
            );
            let delta = c - p;
            let pct = if p > 0.0 { delta / p * 100.0 } else { 0.0 };

            let result = json!({
                "domain": domain,
                "previous": previous,
                "current": current,
                "delta": (delta * 100.0).round() / 100.0,
                "delta_pct": (pct * 10.0).round() / 10.0,
                "direction": if delta > 0.0 { "improved" } else if delta < 0.0 { "regressed" } else { "flat" },
                "summary": format!(
                    "GEO score moved from {p:.0} to {c:.0} between {} and {} ({}{:.0} points).",
                    previous["date"].as_str().unwrap_or(""),
                    current["date"].as_str().unwrap_or(""),
                    if delta >= 0.0 { "+" } else { "" },
                    delta
                ),
            });
            if json {
                print_json(&result)?;
            } else {
                println!("{}", result["summary"].as_str().unwrap_or(""));
            }
            OK
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_normalize_consistently() {
        assert_eq!(normalize_domain("https://www.Example.com/"), "example.com");
        assert_eq!(normalize_domain("example.com"), "example.com");
    }

    #[test]
    fn stage_validation() {
        assert_eq!(check_stage("Proposal-Sent").unwrap(), "proposal_sent");
        assert!(check_stage("nope").is_err());
    }
}

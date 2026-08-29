//! Does the citability score predict citations?
//!
//! Every GEO tool ships a score. None of them show that the score correlates
//! with the thing it claims to predict, because scoring and measuring
//! normally live in different products. Both live in this binary, so the
//! check is possible: score a site's pages, ask the retrieval engines real
//! questions, and see whether the pages they cite are the ones that scored
//! well.
//!
//! The output is deliberately blunt about sample size. A rank correlation
//! over eight pages and five prompts is not evidence, and the report says so
//! rather than printing a confident-looking number.

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::cmd::geo::score_passage;
use crate::cmd::visibility;
use crate::html;
use crate::llm;
use crate::output::{err, print_json, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

#[allow(clippy::too_many_arguments)]
pub fn run(
    domain: &str,
    prompts_file: Option<&str>,
    inline_prompts: &[String],
    provider_spec: Option<&str>,
    max_pages: usize,
    concurrency: usize,
    timeout: u64,
    json: bool,
) -> CmdResult<ExitCode> {
    let providers = llm::resolve(provider_spec).map_err(crate::output::Error)?;
    let retrieval: Vec<_> = providers
        .iter()
        .filter(|p| p.live_search)
        .copied()
        .collect();
    if retrieval.is_empty() {
        return err(
            "this check needs an engine that reports its sources — set PERPLEXITY_API_KEY, \
             or pass --provider perplexity",
        );
    }

    let prompts = super::visibility::prompts_from(prompts_file, inline_prompts)?;
    if prompts.is_empty() {
        return err("no prompts — pass --prompts <file> or repeat --prompt");
    }

    // 1. Score the pages.
    let urls = crate::cmd::core::sitemap_urls_for(domain, max_pages);
    if urls.is_empty() {
        return err(format!("no sitemap URLs found for {domain}"));
    }
    if !json {
        eprintln!("scoring {} pages…", urls.len());
    }
    let mut scores: BTreeMap<String, f64> = BTreeMap::new();
    for url in &urls {
        let rec = crate::cmd::core::fetch_record(url, 30, true, None);
        let Some(body) = rec.content else { continue };
        if rec.status_code.unwrap_or(0) >= 400 {
            continue;
        }
        let blocks = html::content_blocks(&body, 20);
        if blocks.is_empty() {
            continue;
        }
        let avg = blocks
            .iter()
            .map(|b| score_passage(&b.content, Some(&b.heading)).total_score as f64)
            .sum::<f64>()
            / blocks.len() as f64;
        scores.insert(path_key(&rec.url), (avg * 10.0).round() / 10.0);
    }
    if scores.len() < 3 {
        return err(format!(
            "only {} pages produced a score — need at least 3 to compare anything",
            scores.len()
        ));
    }

    // 2. Ask the engines.
    if !json {
        eprintln!(
            "probing {} prompts across {} retrieval engines…",
            prompts.len(),
            retrieval.len()
        );
    }
    let mut jobs = Vec::new();
    for prompt in &prompts {
        for p in &retrieval {
            jobs.push((prompt.as_str(), *p));
        }
    }
    let probes = visibility::probe_all(&jobs, None, timeout, concurrency, !json);

    // 3. Count citations per page.
    let apex = llm::cite_domain_of_host(domain);
    let mut citations: BTreeMap<String, usize> = scores.keys().map(|k| (k.clone(), 0)).collect();
    let mut off_site: BTreeMap<String, usize> = BTreeMap::new();
    let mut answers_with_sources = 0usize;
    for probe in &probes {
        if !probe.ok || probe.citations.is_empty() {
            continue;
        }
        answers_with_sources += 1;
        for c in &probe.citations {
            let is_ours = match (&apex, llm::cite_domain(c)) {
                (Some(a), Some(d)) => d == *a || d.ends_with(&format!(".{a}")),
                _ => false,
            };
            if is_ours {
                let key = path_key(c);
                // A cited URL that was never scored still counts as evidence
                // about the site; it is reported separately rather than
                // silently dropped.
                citations.entry(key).and_modify(|n| *n += 1).or_insert(1);
            } else if let Some(d) = llm::cite_domain(c) {
                *off_site.entry(d).or_insert(0) += 1;
            }
        }
    }

    if answers_with_sources == 0 {
        return err("no answer returned any sources — nothing to correlate");
    }

    // 4. Compare.
    let paired: Vec<(f64, f64)> = scores
        .iter()
        .map(|(k, s)| (*s, citations.get(k).copied().unwrap_or(0) as f64))
        .collect();
    let rho = spearman(&paired);

    let cited: Vec<f64> = scores
        .iter()
        .filter(|(k, _)| citations.get(*k).copied().unwrap_or(0) > 0)
        .map(|(_, s)| *s)
        .collect();
    let uncited: Vec<f64> = scores
        .iter()
        .filter(|(k, _)| citations.get(*k).copied().unwrap_or(0) == 0)
        .map(|(_, s)| *s)
        .collect();

    let mean = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let gap = mean(&cited) - mean(&uncited);

    let mut pages: Vec<Value> = scores
        .iter()
        .map(|(k, s)| json!({"path": k, "citability": s, "times_cited": citations.get(k).copied().unwrap_or(0)}))
        .collect();
    pages.sort_by(|a, b| {
        b["times_cited"]
            .as_u64()
            .cmp(&a["times_cited"].as_u64())
            .then(
                b["citability"]
                    .as_f64()
                    .partial_cmp(&a["citability"].as_f64())
                    .unwrap(),
            )
    });

    let verdict = interpret(rho, gap, cited.len(), scores.len(), answers_with_sources);
    let mut off: Vec<(String, usize)> = off_site.into_iter().collect();
    off.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let doc = json!({
        "domain": domain,
        "pages_scored": scores.len(),
        "prompts": prompts.len(),
        "engines": retrieval.iter().map(|p| p.id).collect::<Vec<_>>(),
        "answers_with_sources": answers_with_sources,
        "pages_cited": cited.len(),
        "mean_citability_cited": round2(mean(&cited)),
        "mean_citability_uncited": round2(mean(&uncited)),
        "gap": round2(gap),
        "spearman_rho": rho.map(round2),
        "verdict": verdict,
        "pages": pages,
        "competing_sources": off.iter().take(15).map(|(d, n)| json!({"domain": d, "citations": n})).collect::<Vec<_>>(),
        "cost_usd": crate::output::money(probes.iter().map(|p| p.cost_usd).sum()),
    });

    if json {
        print_json(&doc)?;
    } else {
        println!(
            "{} pages scored, {} cited by {} sourced answers",
            scores.len(),
            cited.len(),
            answers_with_sources
        );
        println!(
            "mean citability   cited {:.1}  vs  uncited {:.1}   (gap {:+.1})",
            mean(&cited),
            mean(&uncited),
            gap
        );
        match rho {
            Some(r) => println!("Spearman rho      {r:+.2}"),
            None => println!("Spearman rho      n/a — no variation to rank"),
        }
        println!("\n{}", verdict["summary"].as_str().unwrap_or(""));
        println!("{}", verdict["caveat"].as_str().unwrap_or(""));
        if !off.is_empty() {
            println!("\nsources cited instead of you");
            for (d, n) in off.iter().take(10) {
                println!("  {d:<44} {n}");
            }
        }
    }
    OK
}

/// Strip the host so `https://x.test/a` and `https://www.x.test/a/` compare
/// equal — the engines are inconsistent about both.
fn path_key(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let p = u.path().trim_end_matches('/');
            if p.is_empty() {
                "/".to_string()
            } else {
                p.to_string()
            }
        }
        Err(_) => url.to_string(),
    }
}

/// Spearman rank correlation, with ties averaged.
///
/// Rank rather than Pearson because citation counts are small integers with a
/// long zero tail, where a linear correlation is dominated by one outlier.
fn spearman(pairs: &[(f64, f64)]) -> Option<f64> {
    let n = pairs.len();
    if n < 3 {
        return None;
    }
    let xs: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = pairs.iter().map(|p| p.1).collect();
    if all_equal(&xs) || all_equal(&ys) {
        return None;
    }
    let rx = ranks(&xs);
    let ry = ranks(&ys);
    let mean_r = (n as f64 + 1.0) / 2.0;
    let mut num = 0.0;
    let mut dx = 0.0;
    let mut dy = 0.0;
    for i in 0..n {
        let a = rx[i] - mean_r;
        let b = ry[i] - mean_r;
        num += a * b;
        dx += a * a;
        dy += b * b;
    }
    if dx == 0.0 || dy == 0.0 {
        return None;
    }
    Some(num / (dx.sqrt() * dy.sqrt()))
}

fn all_equal(v: &[f64]) -> bool {
    v.windows(2).all(|w| (w[0] - w[1]).abs() < f64::EPSILON)
}

fn ranks(values: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|a, b| {
        values[*a]
            .partial_cmp(&values[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0; values.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && (values[idx[j + 1]] - values[idx[i]]).abs() < f64::EPSILON {
            j += 1;
        }
        // Average rank across the tie block, 1-based.
        let rank = (i + j + 2) as f64 / 2.0;
        for k in i..=j {
            out[idx[k]] = rank;
        }
        i = j + 1;
    }
    out
}

fn interpret(rho: Option<f64>, gap: f64, cited: usize, scored: usize, answers: usize) -> Value {
    let underpowered = cited < 5 || scored < 15 || answers < 10;
    let summary = match rho {
        None => "No usable correlation — the scores or the citation counts had no variation."
            .to_string(),
        Some(r) if r >= 0.4 => format!(
            "Citability tracks citations here (rho {r:+.2}). The pages this tool scores highly are \
             the pages the engines actually quoted."
        ),
        Some(r) if r >= 0.15 => format!(
            "Weak positive relationship (rho {r:+.2}). Citability explains some of what gets cited \
             on this site, not most of it."
        ),
        Some(r) if r > -0.15 => format!(
            "No relationship on this site (rho {r:+.2}). Whatever decides citations here, the \
             passage-level score is not capturing it — check authority and freshness signals."
        ),
        Some(r) => format!(
            "Negative relationship (rho {r:+.2}). Worth investigating: the engines are citing the \
             pages this tool rates worst, which usually means retrieval is driven by something \
             other than page structure."
        ),
    };
    let caveat = if underpowered {
        format!(
            "Underpowered: {cited} cited pages out of {scored} scored, across {answers} sourced \
             answers. Treat this as a direction, not a result — widen --max-pages and the prompt set."
        )
    } else {
        format!("Sample: {scored} pages, {cited} cited, {answers} sourced answers.")
    };
    json!({
        "summary": summary,
        "caveat": caveat,
        "underpowered": underpowered,
        "gap_favours_score": gap > 0.0,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spearman_detects_a_monotonic_relationship() {
        let pairs = vec![(1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)];
        assert!((spearman(&pairs).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn spearman_detects_inversion() {
        let pairs = vec![(1.0, 4.0), (2.0, 3.0), (3.0, 2.0), (4.0, 1.0)];
        assert!((spearman(&pairs).unwrap() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn spearman_needs_variation_on_both_sides() {
        assert!(spearman(&[(1.0, 0.0), (2.0, 0.0), (3.0, 0.0)]).is_none());
        assert!(spearman(&[(1.0, 1.0), (1.0, 2.0)]).is_none());
    }

    #[test]
    fn ties_share_an_averaged_rank() {
        assert_eq!(ranks(&[5.0, 5.0, 9.0]), vec![1.5, 1.5, 3.0]);
    }

    #[test]
    fn path_key_ignores_host_and_trailing_slash() {
        assert_eq!(
            path_key("https://a.test/x/"),
            path_key("https://www.a.test/x")
        );
        assert_eq!(path_key("https://a.test/"), "/");
    }

    #[test]
    fn small_samples_are_flagged_underpowered() {
        let v = interpret(Some(0.9), 10.0, 2, 5, 3);
        assert_eq!(v["underpowered"], json!(true));
    }
}

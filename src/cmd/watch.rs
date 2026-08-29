//! Scheduled monitoring, without a server.
//!
//! A hosted competitor's real advantage is not its dashboard, it is that it
//! runs on Tuesday whether or not anyone remembers. This is the same habit
//! from cron or a CI schedule: one command that runs the checks, writes dated
//! artifacts, and exits non-zero when something got worse — which is the part
//! a scheduler can act on.
//!
//! Each check runs as a subprocess of this same binary so a failure in one is
//! contained, and so the artifacts on disk are byte-identical to what the
//! individual commands produce.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;

use crate::output::{err, print_json, today_utc, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

struct Step {
    name: &'static str,
    argv: Vec<String>,
    /// Non-zero exit means "found something", not "broke".
    finding_exit: i32,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    brand: Option<&str>,
    domain: Option<&str>,
    prompts: Option<&str>,
    competitors: &[String],
    urls: &[String],
    log_files: &[String],
    out_dir: &str,
    html: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    let mut steps: Vec<Step> = Vec::new();

    if let Some(brand) = brand {
        let mut argv = vec![
            "visibility".into(),
            "run".into(),
            "--brand".into(),
            brand.into(),
        ];
        if let Some(d) = domain {
            argv.push("--domain".into());
            argv.push(d.into());
        }
        if let Some(p) = prompts {
            argv.push("--prompts".into());
            argv.push(p.into());
        }
        for c in competitors {
            argv.push("--competitor".into());
            argv.push(c.clone());
        }
        argv.push("--label".into());
        argv.push(format!("watch-{}", today_utc()));
        steps.push(Step {
            name: "visibility",
            argv,
            finding_exit: 0,
        });
        steps.push(Step {
            name: "visibility-diff",
            argv: vec![
                "visibility".into(),
                "diff".into(),
                "--brand".into(),
                brand.into(),
            ],
            finding_exit: 2,
        });
    }

    for url in urls {
        steps.push(Step {
            name: "drift",
            argv: vec!["drift".into(), "compare".into(), url.clone()],
            finding_exit: 2,
        });
    }

    if !log_files.is_empty() {
        let mut argv = vec!["logs".to_string()];
        for f in log_files {
            argv.push(f.clone());
        }
        if let Some(d) = domain {
            argv.push("--sitemap".into());
            argv.push(d.into());
        }
        steps.push(Step {
            name: "logs",
            argv,
            finding_exit: 2,
        });
    }

    if steps.is_empty() {
        return err(
            "nothing to watch — pass --brand (visibility), --url (drift), or --logs (crawler traffic)",
        );
    }

    let dir = PathBuf::from(out_dir).join(today_utc());
    std::fs::create_dir_all(&dir)?;

    let exe = std::env::current_exe()
        .map_err(|e| crate::output::Error(format!("cannot locate seogeo binary: {e}")))?;

    let mut results = Vec::new();
    let mut artifacts: Vec<PathBuf> = Vec::new();
    let mut regressed = false;
    let mut broke = false;

    for (i, step) in steps.iter().enumerate() {
        if !json {
            eprintln!("── {} ({}/{})", step.name, i + 1, steps.len());
        }
        let mut argv = step.argv.clone();
        argv.push("--json".into());
        let output = std::process::Command::new(&exe).args(&argv).output();

        let (code, stdout, stderr) = match output {
            Ok(o) => (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            ),
            Err(e) => (-1, String::new(), e.to_string()),
        };

        let path = unique_path(&dir, step.name);
        let saved = if stdout.trim().is_empty() {
            None
        } else {
            std::fs::write(&path, &stdout)?;
            artifacts.push(path.clone());
            Some(path.display().to_string())
        };

        // Distinguish "the check found a problem" from "the check failed".
        // A scheduler needs both, and conflating them makes every red run
        // look like an outage.
        let finding = step.finding_exit != 0 && code == step.finding_exit;
        let failed = code != 0 && !finding;
        if finding {
            regressed = true;
        }
        if failed {
            broke = true;
        }

        results.push(json!({
            "step": step.name,
            "command": step.argv.join(" "),
            "exit_code": code,
            "status": if failed { "error" } else if finding { "regressed" } else { "ok" },
            "artifact": saved,
            "message": if failed { Some(crate::output::truncate(&stderr, 400)) } else { None },
        }));
    }

    let mut report_path = None;
    if html && !artifacts.is_empty() {
        let target = dir.join("report.html");
        let mut argv: Vec<String> = vec!["report-html".into()];
        for a in &artifacts {
            argv.push("--input".into());
            argv.push(a.display().to_string());
        }
        argv.push("--output".into());
        argv.push(target.display().to_string());
        if let Some(b) = brand {
            argv.push("--title".into());
            argv.push(b.into());
        }
        if std::process::Command::new(&exe)
            .args(&argv)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            report_path = Some(target.display().to_string());
        }
    }

    let doc = json!({
        "date": today_utc(),
        "output_dir": dir.display().to_string(),
        "steps": results,
        "regressed": regressed,
        "errors": broke,
        "report": report_path,
    });

    if json {
        print_json(&doc)?;
    } else {
        println!();
        for r in doc["steps"].as_array().unwrap_or(&vec![]) {
            println!(
                "  {:<16} {}",
                r["step"].as_str().unwrap_or(""),
                r["status"].as_str().unwrap_or("")
            );
        }
        println!("\nartifacts in {}", dir.display());
        if let Some(p) = &report_path {
            println!("report {p}");
        }
    }

    // 2 = something regressed, 1 = a check could not run. A scheduler can act
    // on the first and page on the second.
    if regressed {
        return Ok(ExitCode::from(2));
    }
    if broke {
        return Ok(ExitCode::from(1));
    }
    OK
}

/// `logs.json`, then `logs-2.json` — a watch can run the same check against
/// several targets in one pass.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(format!("{name}.json"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{name}-{n}.json"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_increments_on_collision() {
        let dir = std::env::temp_dir().join("seogeo-watch-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "logs");
        std::fs::write(&a, "{}").unwrap();
        let b = unique_path(&dir, "logs");
        assert_ne!(a, b);
        assert!(b.to_string_lossy().contains("logs-2"));
    }
}

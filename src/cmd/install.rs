//! Skill installer for multiple agent tools.
//!
//! The skills are embedded in the binary, so a user who downloaded only
//! `seogeo` can install them without cloning anything. Each supported tool
//! discovers skills from a different directory and, in Gemini's case, needs
//! an extra manifest; the differences are captured in [`TargetSpec`] rather
//! than scattered through the code.

use std::path::PathBuf;
use std::process::ExitCode;

use include_dir::{include_dir, Dir};
use serde_json::json;

use crate::cli::InstallTarget;
use crate::output::{err, print_json, CmdResult, Error};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills");
static AGENTS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/agents");

/// Where a tool looks for skills, and what else it needs alongside them.
pub struct TargetSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Path relative to the user's home directory.
    pub skills_dir: &'static str,
    /// Path the tool uses to detect that it is installed at all.
    pub probe_dir: &'static str,
    /// Gemini requires a `gemini-extension.json` describing the bundle.
    pub needs_gemini_manifest: bool,
    pub docs: &'static str,
}

pub const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        id: "claude",
        label: "Claude Code",
        skills_dir: ".claude/skills",
        probe_dir: ".claude",
        needs_gemini_manifest: false,
        docs: "https://docs.claude.com/en/docs/claude-code/skills",
    },
    TargetSpec {
        id: "codex",
        label: "OpenAI Codex CLI",
        skills_dir: ".codex/skills",
        probe_dir: ".codex",
        needs_gemini_manifest: false,
        docs: "https://developers.openai.com/codex/",
    },
    TargetSpec {
        id: "gemini",
        label: "Gemini CLI",
        skills_dir: ".gemini/extensions/seo-geo-skill/skills",
        probe_dir: ".gemini",
        needs_gemini_manifest: true,
        docs: "https://google-gemini.github.io/gemini-cli/docs/extensions/",
    },
    TargetSpec {
        id: "opencode",
        label: "OpenCode",
        skills_dir: ".config/opencode/skills",
        probe_dir: ".config/opencode",
        needs_gemini_manifest: false,
        docs: "https://opencode.ai/docs/skills/",
    },
    TargetSpec {
        id: "agents",
        label: "AGENTS.md-compatible agents",
        skills_dir: ".agents/skills",
        probe_dir: ".agents",
        needs_gemini_manifest: false,
        docs: "https://agents.md",
    },
];

fn home() -> CmdResult<PathBuf> {
    dirs::home_dir().ok_or_else(|| Error("could not determine the home directory".into()))
}

fn spec(id: &str) -> Option<&'static TargetSpec> {
    TARGETS.iter().find(|t| t.id == id)
}

fn selected(target: InstallTarget) -> Vec<&'static TargetSpec> {
    match target {
        InstallTarget::Claude => spec("claude").into_iter().collect(),
        InstallTarget::Codex => spec("codex").into_iter().collect(),
        InstallTarget::Gemini => spec("gemini").into_iter().collect(),
        InstallTarget::Opencode => spec("opencode").into_iter().collect(),
        InstallTarget::Agents => spec("agents").into_iter().collect(),
        InstallTarget::All => TARGETS.iter().collect(),
    }
}

fn detected(t: &TargetSpec) -> bool {
    home()
        .map(|h| h.join(t.probe_dir).exists())
        .unwrap_or(false)
}

/// Skill names embedded in this binary.
pub fn skill_names() -> Vec<String> {
    let mut names: Vec<String> = SKILLS
        .dirs()
        .filter(|d| d.get_file(format!("{}/SKILL.md", d.path().display())).is_some())
        .filter_map(|d| d.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

pub fn run(
    target: InstallTarget,
    dir: Option<&str>,
    only: &[String],
    dry_run: bool,
    list: bool,
    json: bool,
) -> CmdResult<ExitCode> {
    if list {
        let rows: Vec<_> = TARGETS
            .iter()
            .map(|t| {
                json!({
                    "target": t.id,
                    "label": t.label,
                    "install_path": home().map(|h| h.join(t.skills_dir).display().to_string()).unwrap_or_default(),
                    "detected": detected(t),
                    "docs": t.docs,
                })
            })
            .collect();
        let out = json!({"skills": skill_names(), "targets": rows});
        if json {
            print_json(&out)?;
        } else {
            println!("{} skill(s) bundled in this binary\n", skill_names().len());
            for r in &rows {
                println!(
                    "  [{}] {:<28} {}",
                    if r["detected"].as_bool().unwrap_or(false) { "detected" } else { "  --    " },
                    r["label"].as_str().unwrap_or(""),
                    r["install_path"].as_str().unwrap_or("")
                );
            }
        }
        return OK;
    }

    let all_names = skill_names();
    if all_names.is_empty() {
        return err("this binary has no embedded skills — rebuild from a full checkout");
    }
    let wanted: Vec<String> = if only.is_empty() {
        all_names.clone()
    } else {
        for name in only {
            if !all_names.contains(name) {
                return err(format!(
                    "unknown skill {name:?}; run `seogeo install --list` to see what is bundled"
                ));
            }
        }
        only.to_vec()
    };

    let mut targets = selected(target);

    // On a machine with no agent tool yet, `--target all` would skip every
    // target and leave the user with a binary and no skills — a silent
    // no-op dressed up as success. Claude Code's directory is the widest
    // fallback: Claude Code and OpenCode both read it.
    let mut fallback_used = false;
    if matches!(target, InstallTarget::All) && dir.is_none() && !targets.iter().any(|t| detected(t)) {
        targets = spec("claude").into_iter().collect();
        fallback_used = true;
    }

    let mut installed = Vec::new();

    for t in &targets {
        // --all installs only where the tool is actually present, so a user
        // running it does not litter their home directory with dead paths.
        if matches!(target, InstallTarget::All) && !fallback_used && !detected(t) && dir.is_none() {
            installed.push(json!({
                "target": t.id, "skipped": true,
                "reason": format!("{} not detected ({} missing)", t.label, t.probe_dir),
            }));
            continue;
        }

        let root = match dir {
            Some(d) => PathBuf::from(d),
            None => home()?.join(t.skills_dir),
        };

        let mut files_written = 0usize;
        for name in &wanted {
            let Some(skill_dir) = SKILLS.get_dir(name.as_str()) else {
                continue;
            };
            files_written += write_dir(skill_dir, &root, name, dry_run)?;
        }

        // Agent definitions ship alongside the skills for the tools that
        // support subagents; the others simply ignore the directory.
        let agents_root = root
            .parent()
            .map(|p| p.join("agents"))
            .unwrap_or_else(|| root.join("agents"));
        let mut agents_written = 0usize;
        for file in AGENTS.files() {
            let Some(name) = file.path().file_name() else { continue };
            let dest = agents_root.join(name);
            if !dry_run {
                std::fs::create_dir_all(&agents_root)?;
                std::fs::write(&dest, file.contents())?;
            }
            agents_written += 1;
        }

        if t.needs_gemini_manifest {
            let manifest_dir = root.parent().unwrap_or(&root).to_path_buf();
            let manifest = json!({
                "name": "seo-geo-skill",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "SEO + GEO analysis skills powered by the seogeo binary.",
                "contextFileName": "GEMINI.md",
                "skills": wanted.iter().map(|n| format!("{n}/*")).collect::<Vec<_>>(),
            });
            if !dry_run {
                std::fs::create_dir_all(&manifest_dir)?;
                std::fs::write(
                    manifest_dir.join("gemini-extension.json"),
                    serde_json::to_string_pretty(&manifest)?,
                )?;
                // The manifest names a context file; shipping the manifest
                // without the file it points at would leave a dangling
                // reference on every Gemini CLI start.
                std::fs::write(manifest_dir.join("GEMINI.md"), gemini_context(&wanted))?;
            }
        }

        installed.push(json!({
            "target": t.id,
            "label": t.label,
            "path": root.display().to_string(),
            "skills": wanted.len(),
            "files": files_written,
            "agents": agents_written,
            "dry_run": dry_run,
        }));
    }

    let binary_on_path = which("seogeo").is_some();
    let result = json!({
        "installed": installed,
        "skills": wanted,
        "no_target_detected": fallback_used,
        "seogeo_on_path": binary_on_path,
        "note": if binary_on_path {
            "The skills call `seogeo`, which is on PATH."
        } else {
            "The skills call `seogeo`, which is NOT on PATH yet — add its directory to PATH or \
             the skills will fail at runtime."
        },
    });

    if json {
        print_json(&result)?;
    } else {
        for i in installed.iter() {
            if i["skipped"] == true {
                println!("skipped: {}", i["reason"].as_str().unwrap_or(""));
                continue;
            }
            println!(
                "{} {} skill(s) -> {}",
                if dry_run { "Would install" } else { "Installed" },
                i["skills"],
                i["path"].as_str().unwrap_or("")
            );
        }
        if fallback_used {
            println!(
                "\nNo agent tool was detected on this machine, so the skills were written to\n\
                 the Claude Code location — Claude Code and OpenCode both read it.\n\
                 For another tool: seogeo install --target codex|gemini|opencode|agents"
            );
        }
        if !binary_on_path {
            println!("\nWarning: `seogeo` is not on PATH. The skills invoke it by name.");
        }
    }
    OK
}

/// Context the Gemini CLI loads with the extension. Short on purpose: it is
/// in context for every turn, so it says only what the model cannot discover
/// from the skills themselves.
fn gemini_context(skills: &[String]) -> String {
    const BODY: &str = r#"# SEO + GEO skills

{COUNT} skills for auditing a site for classic search and for answer engines
(ChatGPT, Claude, Perplexity, Gemini, Google AI Overviews).

Every skill executes through the `seogeo` binary — one static executable, no
Python. If a command reports that `seogeo` is not found, the binary is missing
from PATH; see the `seo-geo-skill` skill for the install steps.

Useful entry points:

- `seogeo commands` — every subcommand and the skills that use it
- `seogeo <command> --help` — flags for one command
- `seogeo google-auth --check` — which API credentials are configured

Pass `--json` whenever you will parse the output; the human-readable form is
lossy by design.
"#;
    BODY.replace("{COUNT}", &skills.len().to_string())
}

/// Copy an embedded skill directory, preserving `references/`, `templates/`,
/// and any other nested resources the skill ships.
fn write_dir(
    dir: &Dir<'_>,
    root: &std::path::Path,
    skill_name: &str,
    dry_run: bool,
) -> CmdResult<usize> {
    let mut count = 0;
    for file in dir.files() {
        let rel = file
            .path()
            .strip_prefix(skill_name)
            .unwrap_or(file.path());
        let dest = root.join(skill_name).join(rel);
        if !dry_run {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, file.contents())?;
        }
        count += 1;
    }
    for sub in dir.dirs() {
        count += write_dir(sub, root, skill_name, dry_run)?;
    }
    Ok(count)
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_has_a_distinct_path() {
        let mut paths: Vec<&str> = TARGETS.iter().map(|t| t.skills_dir).collect();
        let n = paths.len();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), n);
    }

    #[test]
    fn skills_are_embedded() {
        let names = skill_names();
        assert!(names.len() >= 40, "expected the full skill set, got {}", names.len());
        assert!(names.iter().any(|n| n.starts_with("geo-")));
        assert!(names.iter().any(|n| n.starts_with("seo-")));
    }

    #[test]
    fn every_embedded_skill_has_frontmatter() {
        for name in skill_names() {
            let dir = SKILLS.get_dir(name.as_str()).unwrap();
            let file = dir
                .get_file(format!("{name}/SKILL.md"))
                .unwrap_or_else(|| panic!("{name} has no SKILL.md"));
            let text = std::str::from_utf8(file.contents()).unwrap();
            assert!(text.starts_with("---\n"), "{name}: SKILL.md has no frontmatter");
            assert!(text.contains("\nname:"), "{name}: frontmatter has no name");
            assert!(
                text.contains("\ndescription:"),
                "{name}: frontmatter has no description"
            );
        }
    }
}

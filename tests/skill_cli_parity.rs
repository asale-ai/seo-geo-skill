//! Parity gate between the shipped skills and the CLI.
//!
//! A skill that calls a subcommand the binary does not have fails at runtime
//! in front of a user, and a subcommand no skill calls is dead weight. Both
//! directions are checked here so the two cannot drift apart silently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_seogeo");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(markdown_files(&path));
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    out
}

/// Subcommands the binary actually exposes, taken from `--help` so the test
/// reads the real clap definition rather than a copy of it.
fn cli_commands() -> BTreeSet<String> {
    let out = Command::new(BIN)
        .arg("--help")
        .output()
        .expect("seogeo --help should run");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commands = BTreeSet::new();
    let mut in_commands = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.starts_with("Options:") || line.trim().is_empty() && !commands.is_empty() {
                if line.starts_with("Options:") {
                    break;
                }
                continue;
            }
            let trimmed = line.trim_start();
            if line.starts_with(char::is_alphabetic) {
                break;
            }
            if let Some(word) = trimmed.split_whitespace().next() {
                if !word.is_empty()
                    && word.starts_with(|c: char| c.is_ascii_lowercase())
                    && word
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    commands.insert(word.to_string());
                }
            }
        }
    }
    commands.remove("help");
    assert!(
        commands.len() > 40,
        "parsed too few commands from --help: {commands:?}"
    );
    commands
}

/// Every `seogeo <command>` mentioned in a skill or agent file, mapped to the
/// files that mention it.
fn skill_invocations() -> BTreeMap<String, BTreeSet<String>> {
    let re = regex_lite(r"seogeo ([a-z][a-z0-9-]*)");
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for dir in ["skills", "agents"] {
        for path in markdown_files(&repo_root().join(dir)) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let label = path
                .strip_prefix(repo_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            for cmd in re(&text) {
                found.entry(cmd).or_default().insert(label.clone());
            }
        }
    }
    found
}

/// Extract `seogeo <word>` occurrences, but only from code — fenced blocks
/// and inline spans. Prose like "installs the seogeo binary" is documentation,
/// not an invocation, and must not be mistaken for one.
fn regex_lite(_pattern: &str) -> impl Fn(&str) -> Vec<String> {
    |text: &str| {
        let mut code = String::new();
        let mut in_fence = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                code.push_str(line);
                code.push('\n');
                continue;
            }
            // Inline spans on a prose line.
            let mut rest = line;
            while let Some(open) = rest.find('`') {
                let after = &rest[open + 1..];
                match after.find('`') {
                    Some(close) => {
                        code.push_str(&after[..close]);
                        code.push('\n');
                        rest = &after[close + 1..];
                    }
                    None => break,
                }
            }
        }

        let mut out = Vec::new();
        let needle = "seogeo ";
        let mut rest = code.as_str();
        while let Some(idx) = rest.find(needle) {
            let after = &rest[idx + needle.len()..];
            let word: String = after
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .collect();
            if !word.is_empty() {
                out.push(word);
            }
            rest = &rest[idx + needle.len()..];
        }
        out
    }
}

/// Flags that follow `seogeo ` in prose (`seogeo --version`) are not commands.
fn is_command_like(word: &str) -> bool {
    !word.is_empty() && !word.starts_with('-')
}

#[test]
fn every_skill_invocation_resolves_to_a_real_command() {
    let cli = cli_commands();
    let invocations = skill_invocations();

    let mut unknown: Vec<String> = Vec::new();
    for (cmd, files) in &invocations {
        if !is_command_like(cmd) {
            continue;
        }
        if !cli.contains(cmd) {
            unknown.push(format!(
                "  `seogeo {cmd}` (in {})",
                files.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    assert!(
        unknown.is_empty(),
        "skills invoke subcommands the CLI does not have:\n{}",
        unknown.join("\n")
    );
}

#[test]
fn every_command_is_reachable_from_a_skill() {
    let cli = cli_commands();
    let invoked: BTreeSet<String> = skill_invocations().into_keys().collect();

    // These exist for operators, not for skills to call mid-analysis.
    let operator_only: BTreeSet<&str> = ["install", "commands", "sync-flow"].into_iter().collect();

    let orphans: Vec<&String> = cli
        .iter()
        .filter(|c| !invoked.contains(*c) && !operator_only.contains(c.as_str()))
        .collect();

    assert!(
        orphans.is_empty(),
        "CLI subcommands no skill or agent documents: {orphans:?}"
    );
}

#[test]
fn command_map_covers_the_cli() {
    let cli = cli_commands();
    let out = Command::new(BIN)
        .args(["commands", "--json"])
        .output()
        .expect("seogeo commands --json should run");
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("commands --json should emit JSON");
    let mapped: BTreeSet<String> = json["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .filter_map(|c| c["command"].as_str().map(String::from))
        .collect();

    let missing: Vec<&String> = cli.difference(&mapped).collect();
    assert!(
        missing.is_empty(),
        "`seogeo commands` omits: {missing:?} — add them to COMMAND_MAP"
    );

    let stale: Vec<&String> = mapped.difference(&cli).collect();
    assert!(
        stale.is_empty(),
        "`seogeo commands` lists commands that no longer exist: {stale:?}"
    );
}

#[test]
fn every_skill_has_well_formed_frontmatter() {
    let mut problems = Vec::new();
    let skills_dir = repo_root().join("skills");
    let entries = std::fs::read_dir(&skills_dir).expect("skills/ should exist");

    let mut count = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let skill_md = dir.join("SKILL.md");
        if !skill_md.exists() {
            problems.push(format!("{name}: no SKILL.md"));
            continue;
        }
        count += 1;
        let text = std::fs::read_to_string(&skill_md).unwrap();
        if !text.starts_with("---\n") {
            problems.push(format!("{name}: SKILL.md does not open with frontmatter"));
            continue;
        }
        let Some(end) = text[4..].find("\n---") else {
            problems.push(format!("{name}: frontmatter is never closed"));
            continue;
        };
        let front = &text[4..4 + end];
        if !front.contains("name:") {
            problems.push(format!("{name}: frontmatter has no `name`"));
        }
        if !front.contains("description:") {
            problems.push(format!("{name}: frontmatter has no `description`"));
        }
        // The directory name is the identifier every tool keys on.
        let declared = front
            .lines()
            .find(|l| l.starts_with("name:"))
            .map(|l| l["name:".len()..].trim().trim_matches('"').to_string());
        if declared.as_deref() != Some(name.as_str()) {
            problems.push(format!(
                "{name}: frontmatter name is {declared:?}, expected {name:?}"
            ));
        }
    }

    assert!(count >= 40, "expected the full skill set, found {count}");
    assert!(
        problems.is_empty(),
        "skill frontmatter problems:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn no_skill_still_references_a_python_script() {
    let mut offenders = Vec::new();
    for dir in ["skills", "agents"] {
        for path in markdown_files(&repo_root().join(dir)) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                // `.py` in prose is fine; an invocation is not.
                let looks_like_call = line.contains("python3 ")
                    || line.contains("python ")
                    || (line.contains(".py") && line.contains("scripts/"));
                if looks_like_call {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "skills must call the seogeo binary, not Python:\n  {}",
        offenders.join("\n  ")
    );
}

/// Split a documented shell line into argv, honouring quotes and stopping at a
/// comment, pipe, or redirect. Deliberately small: the goal is to read the
/// flags a skill documents, not to be a shell.
fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), ch) if ch == q => quote = None,
            (Some(_), ch) => cur.push(ch),
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '#') | (None, '|') | (None, '>') => break,
            (None, '\\') => {
                // A trailing backslash continues the line; anything else is an
                // escape we can drop.
                if chars.peek().is_none() {
                    break;
                }
            }
            (None, ch) if ch.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, ch) => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every `seogeo …` command line documented in a skill or agent, one per entry,
/// paired with the file and line it came from.
fn documented_command_lines() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["skills", "agents"] {
        for path in markdown_files(&repo_root().join(dir)) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let label = path
                .strip_prefix(repo_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            let mut in_fence = false;
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                let mut candidates: Vec<String> = Vec::new();
                if in_fence && line.contains("seogeo ") {
                    candidates.push(line.trim().to_string());
                } else {
                    let mut rest = line;
                    while let Some(open) = rest.find('`') {
                        let after = &rest[open + 1..];
                        let Some(close) = after.find('`') else { break };
                        let span = &after[..close];
                        if span.contains("seogeo ") {
                            candidates.push(span.to_string());
                        }
                        rest = &after[close + 1..];
                    }
                }
                for c in candidates {
                    if let Some(idx) = c.find("seogeo ") {
                        out.push((format!("{label}:{}", n + 1), c[idx..].to_string()));
                    }
                }
            }
        }
    }
    out
}

fn help_text(argv: &[String]) -> String {
    let out = Command::new(BIN)
        .args(argv)
        .arg("--help")
        .output()
        .expect("seogeo --help should run");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn flags_in(help: &str) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    let mut word = String::new();
    let mut chars = help.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek() == Some(&'-') {
            chars.next();
            word.clear();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_lowercase() || n.is_ascii_digit() || n == '-' {
                    word.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if !word.is_empty() {
                flags.insert(format!("--{word}"));
            }
        }
    }
    flags
}

fn subcommands_in(help: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut seen = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            seen = true;
            continue;
        }
        if seen {
            if line.starts_with("Options:") || line.starts_with(char::is_alphabetic) {
                break;
            }
            if let Some(word) = line.split_whitespace().next() {
                if word.starts_with(|c: char| c.is_ascii_lowercase()) {
                    out.insert(word.to_string());
                }
            }
        }
    }
    out
}

/// A skill that documents a flag the CLI does not accept produces a usage
/// error in front of a user, which is worse than no documentation at all.
#[test]
fn documented_flags_exist() {
    let mut problems = Vec::new();

    for (where_, line) in documented_command_lines() {
        let tokens = tokenize(&line);
        if tokens.len() < 2 || tokens[0] != "seogeo" {
            continue;
        }
        let mut argv = vec![tokens[1].clone()];
        if argv[0].starts_with('-') {
            continue; // `seogeo --version`
        }
        let mut rest = &tokens[2..];

        // Descend one level into subcommand groups (drift baseline, moz metrics…).
        let help = help_text(&argv);
        if let Some(first) = rest.first() {
            if subcommands_in(&help).contains(first) {
                argv.push(first.clone());
                rest = &rest[1..];
            }
        }

        let valid = flags_in(&help_text(&argv));
        for tok in rest {
            if !tok.starts_with("--") {
                continue;
            }
            let name = tok.split('=').next().unwrap_or(tok);
            // Placeholders inside angle brackets are prose, not flags.
            if name.contains('<') || name.contains('>') {
                continue;
            }
            if !valid.contains(name) {
                problems.push(format!(
                    "{where_}: `seogeo {}` has no {name}\n      in: {line}",
                    argv.join(" ")
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "skills document flags the CLI does not accept:\n  {}",
        problems.join("\n  ")
    );
}

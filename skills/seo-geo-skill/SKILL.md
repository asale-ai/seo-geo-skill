---
name: seo-geo-skill
description: Bootstrap the SEO + GEO skill suite — installs the seogeo binary and writes all 48 SEO/GEO skills into every agent tool on this machine. Use when the user has just installed this skill and nothing else works yet, or says "set up seo skills", "install seogeo", "seogeo not found", "bootstrap geo skills", or asks why an SEO/GEO command is missing.
user-invocable: true
license: MIT
metadata:
  category: setup
  version: "0.1.0"
---

# Bootstrap the SEO + GEO suite

This skill exists to install the rest. The 48 SEO and GEO skills all execute
through one binary, `seogeo`; without it they cannot run. Installing this skill
from ClawHub gets you this file — running it gets you everything else.

---

## Step 1 — Check what is already here

```bash
seogeo --version && seogeo install --list
```

Three outcomes:

| Result | What it means | Do this |
|--------|---------------|---------|
| Prints a version and a target list | Already installed | Go to step 3 |
| `command not found` | Binary missing | Step 2 |
| Prints a version but `install --list` shows no detected targets | Binary present, no agent tool found | Step 3 with an explicit `--target` |

---

## Step 2 — Install the binary

**Show the user the command and get their agreement before running it.** It
downloads and executes a binary from a GitHub release; that is a decision for
them to make, not for you to make on their behalf.

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.ps1 | iex
```

The script resolves the latest release, verifies the archive against the
published `SHA256SUMS`, installs to `~/.local/bin` (`%LOCALAPPDATA%\Programs\seogeo`
on Windows), and then installs the skills. If checksum verification fails it
aborts without touching anything — report that failure verbatim and stop.

If the user declines the script, `cargo install --git https://github.com/asale-ai/seo-geo-skill`
builds it from source instead.

---

## Step 3 — Install the skills

```bash
seogeo install --target npx
```

This delegates to [`npx skills`](https://github.com/vercel-labs/skills), which
supports 75+ agents. It keeps one canonical copy in `~/.agents/skills` and
symlinks it into each agent that is present, and it is pinned to the binary's
own tag so the skills always match the code.

If Node is not available, write the directories directly instead — this needs
no network and no npm:

```bash
seogeo install --target all
```

Preview either one first:

```bash
seogeo install --target npx --dry-run --json
seogeo install --target all --dry-run --json
```

To target a single tool:

```bash
seogeo install --target claude     # ~/.claude/skills
seogeo install --target codex      # ~/.codex/skills
seogeo install --target gemini     # ~/.gemini/extensions/seo-geo-skill
seogeo install --target opencode   # ~/.config/opencode/skills
seogeo install --target agents     # ~/.agents/skills
```

---

## Step 4 — Confirm, then tell the user what they can do

```bash
seogeo install --list
seogeo commands | head -20
```

If `seogeo` is not on PATH, the installer says so and prints the line to add.
Relay that instead of working around it — the skills invoke `seogeo` by name,
so a PATH gap makes all 48 fail at the first command.

Claude Code discovers new skills at session start, so mention that a restart
may be needed.

Then tell them what is now possible, concretely:

```
/geo audit <url>          Full GEO + SEO audit with a prioritised plan
/geo citability <url>     Which passages an answer engine would quote
/geo crawlers <url>       Whether AI crawlers are allowed and actually served
/seo audit <url>          Full technical + content + schema audit
/seo technical <url>      Crawlability, Core Web Vitals, security headers
/seo drift baseline <url> Catch regressions between deploys
```

---

## What works immediately, and what needs keys

Most commands need nothing: page fetching, SEO parsing, citability scoring,
robots and AI-crawler checks, `llms.txt`, schema validation and generation,
content quality, hreflang, image audits, drift monitoring, WHOIS heritage,
IndexNow, backlink verification, Common Crawl ranks, PDF reports.

Field data and account data need credentials. Check before promising anything:

```bash
seogeo google-auth --check
seogeo backlinks-auth --check
```

Two optional binaries widen the range: **Chrome** (Chrome, Chromium, or Edge)
for rendering, screenshots, and PDFs; **Node** for the Unlighthouse sweep. Both
are auto-detected, and the commands that need them say so when they are
missing.

---

## Updating later

```bash
seogeo --version
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
seogeo install --target npx
```

The `geo-update` skill walks this properly, including the version comparison
and the list of user state that survives an upgrade.

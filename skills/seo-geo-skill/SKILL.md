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
downloads and runs a binary from a GitHub release; that is a decision for them
to make, not for you to make on their behalf.

If Node is available — every platform, one command:

```bash
npx -y @asale/seogeo --version
```

That resolves the package, downloads the release build for the machine's
platform, verifies it against the published `SHA256SUMS`, and runs it. To keep
`seogeo` on the PATH rather than going through `npx` each time:

```bash
npm install -g @asale/seogeo
```

The npm version and the release tag are the same number, so
`@asale/seogeo@0.2.0` can only ever fetch `v0.2.0`.

No Node? The shell installers do the same job:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.ps1 | iex
```

Either route verifies the archive against `SHA256SUMS` and aborts without
touching anything if verification fails — report that failure verbatim and
stop. The shell installers put the binary in `~/.local/bin`
(`%LOCALAPPDATA%\Programs\seogeo` on Windows) and install the skills as well.

If the user declines both, `cargo install seogeo` builds it from source.

---

## Step 3 — Install the skills

```bash
seogeo install --target npx
```

This delegates to [`npx skills`](https://github.com/vercel-labs/skills), which
supports 75+ agents. Note the two are unrelated: `@asale/seogeo` delivers the
binary, `npx skills` distributes the skill files. It keeps one canonical copy in `~/.agents/skills` and
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

## Using it from a host that does not read skills

The skills reach agents that load `SKILL.md` files. Everything else — Cursor,
Windsurf, Claude Desktop, n8n, a custom agent — reaches the same checks over
MCP, from the same binary:

```bash
seogeo mcp
```

It speaks JSON-RPC on stdio and exposes the audit, crawl, visibility, and log
tools. Register it the way that host registers any stdio MCP server; the
command is `seogeo` with the single argument `mcp` and no environment beyond
whatever API keys the user already set.

Two things worth telling the user:

- The tool list is deliberately short. Each tool is a whole check that returns a finished report, not an API endpoint to be assembled — a model choosing between 14 outcomes picks better than one choosing between 160 primitives.
- Answer-engine tools still need the user's own keys. `ai_ask` and `ai_visibility_run` return a readable error naming the missing variable rather than failing silently.

Prefer the skills where both are available: a skill carries the judgement about
what to do with the numbers, and the MCP tool only carries the numbers.

---

## Updating later

```bash
seogeo --version
npx -y @asale/seogeo@latest --version
seogeo install --target npx
```

If the binary came from the shell installer rather than npm, re-run that
instead:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

The `geo-update` skill walks this properly, including the version comparison
and the list of user state that survives an upgrade.

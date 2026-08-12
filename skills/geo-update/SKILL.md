---
name: geo-update
description: Update the installed SEO + GEO skills and the seogeo CLI to the latest release. Reports the installed version, the latest published version, what changed, and reinstalls the skill set across every detected agent tool. Use when the user says "update geo", "update skills", "upgrade seogeo", "aggiorna", or asks whether a newer version exists.
allowed-tools:
  - Bash
  - Read
metadata:
  category: geo
---

# Update the SEO + GEO toolkit

The skills and the `seogeo` binary version together. Updating means
upgrading the binary, then reinstalling the skills that ship inside it.

---

## Step 1 — Report the current state

```bash
seogeo --version
seogeo install --list --json
```

`install --list` prints every supported agent tool, whether it is detected
on this machine, and where its skills live. Show the user the installed
version and the detected targets before changing anything.

If `seogeo` is not found, the toolkit is not installed. Point the user at
the install command rather than trying to repair the installation:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

---

## Step 2 — Check for a newer release

```bash
curl -fsSL https://api.github.com/repos/asale-ai/seo-geo-skill/releases/latest \
  | grep '"tag_name"'
```

Compare against `seogeo --version`. If they match, tell the user they are
current and stop — do not reinstall for no reason.

---

## Step 3 — Upgrade the binary

The install script is idempotent and replaces the binary in place:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.ps1 | iex
```

The script verifies the release checksum before installing. If verification
fails it aborts without touching the existing binary — report the failure
verbatim and stop.

---

## Step 4 — Reinstall the skills

The new binary carries the new skills:

```bash
seogeo install --target all
```

This writes to every agent tool detected on the machine (Claude Code,
Codex CLI, Gemini CLI, OpenCode, and any AGENTS.md-based agent) and skips
the ones that are not present. To target one tool:

```bash
seogeo install --target claude
seogeo install --target codex
```

Preview without writing:

```bash
seogeo install --target all --dry-run --json
```

---

## Step 5 — Verify

```bash
seogeo --version
seogeo commands --json | head -20
seogeo install --list
```

Confirm the version advanced and that the skill count matches what
`install --list` reports. Tell the user which tools were updated and remind
them that Claude Code picks up new skills on the next session start.

---

## What is preserved

Updating never touches user state. These survive an upgrade:

| State | Location |
|-------|----------|
| Drift baselines | `~/.cache/seogeo/drift/baselines.db` |
| CRM prospects | `<data dir>/seogeo/prospects.json` |
| DataForSEO cost ledger | `<data dir>/seogeo/dataforseo-costs.json` |
| Google API config | `~/.config/seogeo/google-api.json` |
| Backlink API config | `~/.config/seogeo/backlinks-api.json` |

Skill files are overwritten. If the user edited a bundled skill, tell them
to copy it to a new name before updating.

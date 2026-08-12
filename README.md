<!-- SPDX-License-Identifier: MIT -->

# SEO + GEO Skills

[![CI](https://github.com/asale-ai/seo-geo-skill/actions/workflows/ci.yml/badge.svg)](https://github.com/asale-ai/seo-geo-skill/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/asale-ai/seo-geo-skill?sort=semver)](https://github.com/asale-ai/seo-geo-skill/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/asale-ai/seo-geo-skill/total)](https://github.com/asale-ai/seo-geo-skill/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](https://github.com/asale-ai/seo-geo-skill/releases/latest)

Agent skills that audit a website for classic search **and** for the answer
engines — ChatGPT, Claude, Perplexity, Gemini, Google AI Overviews.

48 skills, 23 subagents, and one static binary that does the actual work. No
Python, no virtualenv, no `pip install`.

---

## Install

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.ps1 | iex
```

That installs the `seogeo` binary — checksum-verified against the published
`SHA256SUMS` — and then hands the skills to
[`npx skills`](https://github.com/vercel-labs/skills), which supports **75+
agents**: Claude Code, Codex, Cursor, OpenCode, Cline, Copilot, Roo, Windsurf,
and the rest. One canonical copy lands in `~/.agents/skills` and is symlinked
into each agent that is present. The install is pinned to the same tag as the
binary, so the skills can never be newer or older than the code that runs them.

No Node? The installer falls back to writing five known agent directories
directly. Nothing here requires npm.

Confirm it worked:

```bash
seogeo --version
seogeo install --list
```

<details>
<summary>Other ways in</summary>

**Skills only, no binary yet** — the bundled `seo-geo-skill` skill will then
walk your agent through installing the binary:

```bash
npx skills add asale-ai/seo-geo-skill --all
```

**ClawHub:**

```bash
clawhub install @asale-ai/seo-geo-skill
```

**From source:**

```bash
cargo install --git https://github.com/asale-ai/seo-geo-skill
seogeo install --target npx      # or --target all
```

**Pick your own targets:**

```bash
seogeo install --target npx        # 75+ agents via npx skills
seogeo install --target all        # the five paths seogeo writes itself
seogeo install --target claude     # just one
seogeo install --list              # what is detected here
```

</details>

---

## Using the skills

Ask for what you want in plain language. The skill descriptions are written so
your agent picks the right one; you rarely need to name it.

```
Audit https://example.com for AI search visibility
Why isn't my product page getting cited by ChatGPT?
Check whether AI crawlers can actually reach my site
Generate an llms.txt for example.com
Did a Google update cause my traffic drop last month?
```

In Claude Code you can also invoke them directly:

| Command | What you get |
|---------|--------------|
| `/geo audit <url>` | Full GEO + SEO audit with a composite score and a prioritised plan |
| `/geo citability <url>` | Passage-by-passage score for how quotable the page is to an answer engine |
| `/geo crawlers <url>` | Whether each AI crawler is allowed **and** actually served |
| `/geo llmstxt <url>` | Validate an existing `llms.txt`, or generate one |
| `/geo brands <url>` | Brand presence on the platforms AI answers cite most |
| `/geo report-pdf` | Turn the audit into a client-ready PDF |
| `/seo audit <url>` | Full technical + content + schema audit |
| `/seo page <url>` | Deep single-page analysis |
| `/seo technical <url>` | Crawlability, indexability, Core Web Vitals, security headers |
| `/seo content <url>` | E-E-A-T, thin content, uncited claims, AI-pattern density |
| `/seo schema <url>` | Detect, validate, and generate structured data |
| `/seo images <url>` | Alt text, dimensions, formats, lazy loading, LCP impact |
| `/seo sitemap <url>` | Sitemap discovery and validation |
| `/seo hreflang <url>` | International SEO annotations |
| `/seo local <url>` | Google Business Profile, NAP, citations, local schema |
| `/seo backlinks <url>` | Link profile, and verification that claimed links exist |
| `/seo drift baseline\|compare <url>` | Catch SEO regressions between deploys |
| `/seo google <command>` | Search Console, PageSpeed, CrUX, GA4, Indexing API |

`seogeo commands` lists every subcommand and the skills that use it.

---

## What works without any setup

Most of it. These need no accounts, no keys, and no quota:

page fetching and rendering · SEO element extraction · AI-citability scoring ·
robots.txt and live AI-crawler checks · `llms.txt` validation and generation ·
schema detection, validation, and generation · content quality and
citation-gap analysis · hreflang · image audits · Core Web Vitals *lab* signals ·
speculation rules and bfcache · drift monitoring · WHOIS heritage · parasite-SEO
risk · IndexNow submission · backlink verification · Common Crawl ranks ·
PDF report generation · the CRM pipeline.

Field data and account data need credentials. Check what you have:

```bash
seogeo google-auth --check
seogeo backlinks-auth --check
```

| Set this | And these start working |
|----------|-------------------------|
| `GOOGLE_API_KEY` | PageSpeed Insights, CrUX field data, LCP subparts, Natural Language, YouTube |
| `GOOGLE_APPLICATION_CREDENTIALS` | Search Console, URL Inspection, Indexing API, GA4 |
| `MOZ_API_KEY` | Domain Authority, referring domains, anchor text |
| `BING_WEBMASTER_API_KEY` | Bing link data, IndexNow |
| `DATAFORSEO_LOGIN` + `DATAFORSEO_PASSWORD` | Live SERPs, keyword volume, marketplace data |

`seogeo google-auth --setup` prints the exact steps for the Google side.

Two optional binaries widen what is possible: **Chrome** (any of Chrome,
Chromium, or Edge) for rendering, screenshots, and PDFs, and **Node** for the
Unlighthouse sweep. Both are detected automatically, and the commands that need
them say so plainly when they are missing.

---

## Behind a restrictive network?

If requests fail while `curl` succeeds, point the tool at your proxy:

```bash
export SEOGEO_PROXY=http://127.0.0.1:7890
```

`HTTPS_PROXY` and `ALL_PROXY` are honoured too.

---

## Uninstall

```bash
npx skills remove          # if the skills came from npx skills
rm -rf ~/.claude/skills/{seo,geo}* ~/.claude/agents/{seo,geo}*-*.md
rm -f ~/.local/bin/seogeo
```

Your data — drift baselines, CRM records, cost ledger — lives under
`~/.config/seogeo`, `~/.local/share/seogeo`, and `~/.cache/seogeo`. Remove those
too if you want a clean slate.

---

## Prior art

This project did not start from a blank page. The skill taxonomy, the scoring
models, and much of the domain guidance come from two MIT-licensed projects:

| Project | Author | What it contributed here |
|---------|--------|--------------------------|
| [AgriciDaniel/claude-seo](https://github.com/AgriciDaniel/claude-seo) | agricidaniel | The `seo-*` skill family, the SEO subagents, the reference library, the schema templates |
| [zubair-trabzada/geo-seo-claude](https://github.com/zubair-trabzada/geo-seo-claude) | Zubair Trabzada | The `geo-*` skill family, the GEO subagents, the citability and brand-authority scoring, the report templates |

Both are Python-and-Claude-Code projects; the rewrite here moves every
execution step onto the `seogeo` binary and widens the install to 75+ agents.
If you want the originals — including the parts this port left behind — go read
them. [Attribution](THIRD-PARTY-NOTICES.md) records the full chain, including
the upstream community contributors whose credit travels with the code.

---

## Links

[Contributing](CONTRIBUTING.md) ·
[Security](SECURITY.md) ·
[Attribution](THIRD-PARTY-NOTICES.md) ·
[MIT](LICENSE)

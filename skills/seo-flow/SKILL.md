---
name: seo-flow
description: >
  FLOW framework integration: evidence-led SEO using the Find → Leverage →
  Optimize → Win loop. Surfaces stage-specific AI prompts from the FLOW
  knowledge base (41 prompts, CC BY 4.0). Use when user says "FLOW", "FLOW
  framework", "seo flow", "evidence-led SEO", "find leverage optimize win",
  or wants stage-specific SEO prompts.
user-invocable: true
argument-hint: "[stage] [url|topic]"
license: MIT
metadata:
  author: AgriciDaniel
  version: "2.2.4"
  category: seo
---

# FLOW Framework: Find · Leverage · Optimize · Win

FLOW is an evidence-led SEO operating model built for the AI-search era. SEO + GEO Skills
integrates the FLOW prompt library (41 prompts across 5 stages) so every analysis can
be driven by structured, evidence-backed AI prompts rather than improvised queries.

> Framework and prompts © Daniel Agrici, CC BY 4.0: github.com/AgriciDaniel/flow

**Runtime context:** Load `references/flow-framework.md` on every `/seo flow` activation.
Load prompt files on demand, only for the stage the user requests.

---

## Commands

| Command | What it does |
|---------|-------------|
| `/seo flow` | Show FLOW overview + stage menu |
| `/seo flow find [url\|topic]` | Find-stage: keyword research, gap analysis, SERP intent mapping (5 prompts) |
| `/seo flow leverage [url]` | Leverage-stage: backlink strategy, off-site authority (1 prompt) |
| `/seo flow optimize [url]` | Optimize-stage: select 2-3 most relevant of 21 prompts based on context |
| `/seo flow win [url]` | Win-stage: BOFU, conversion rate, dual-surface scorecard (3 prompts) |
| `/seo flow local [url]` | Local-stage: GBP optimization, meta, title tags, local audits (11 prompts) |
| `/seo flow prompts` | Full index of all 41 prompts (stage, name, trigger conditions) |
| `/seo flow sync` | Pull latest prompt files from github.com/AgriciDaniel/flow |

---

## Orchestration Logic

### On `/seo flow` (no sub-command)
1. Read `references/flow-framework.md`
2. Show the FLOW stage overview with a one-line description of each stage
3. Ask: which stage matches the user's current situation?

### On `/seo flow find [url|topic]`
1. Read all files in `references/prompts/find/`
2. Apply each prompt to the URL or topic
3. Cross-reference: "For deeper SERP clustering, see `/seo cluster <seed-keyword>`"

### On `/seo flow leverage [url]`
1. Read the file in `references/prompts/leverage/`
2. Apply to the URL's current backlink context
3. Cross-reference: "For raw backlink data, see `/seo backlinks <url>`"

### On `/seo flow optimize [url]`
1. Read all file names in `references/prompts/optimize/`
2. Read prior analysis context (URL, industry vertical, any prior skill output in conversation)
3. Select 2-3 most relevant prompts; load only those files
4. Apply selected prompts; note the others are accessible via `/seo flow prompts`
5. Cross-reference: "For full content quality analysis, see `/seo content <url>` and `/seo geo <url>`"

### On `/seo flow win [url]`
1. Read all files in `references/prompts/win/`
2. Apply each prompt to the URL's conversion and BOFU context
3. Cross-reference: "For SXO persona scoring, see `/seo sxo <url>`"

### On `/seo flow local [url]`
1. Read all files in `references/prompts/local/`
2. Apply to the URL's local SEO context
3. Cross-reference: "For full local SEO analysis, see `/seo local <url>` and `/seo maps [command]`"

### On `/seo flow prompts`
1. Read `references/prompts/README.md`
2. Display the full index: all 41 prompts with stage, name, trigger conditions

### On `/seo flow sync`

The prompt library lives in a third-party repository (CC BY 4.0), not in this
skill. Syncing is optional — the FLOW *method* below works without it; only the
41 verbatim prompts need the sync.

1. Preview first: `seogeo sync-flow --dry-run --json`
2. Then: `seogeo sync-flow --json`
3. Display the summary (`available`, `written`, `destination`)
4. Every synced file carries a CC BY 4.0 attribution header; say so

If the sync returns HTTP 404, the upstream repository has moved or gone
private. Do not retry in a loop and do not present the framework as broken —
say the prompt library is unavailable, and continue with the stage guidance in
this file. A user with a fork can point at it:

```bash
SEOGEO_FLOW_REPO=owner/name seogeo sync-flow --json
```

---

## Context Matching (Optimize stage)

The optimize stage has 21 prompts. Dumping all 21 is noise. Select by priority:

1. **Industry vertical** (SaaS → on-page + technical; local → citations + GBP; publisher → E-E-A-T + freshness)
2. **Prior skill output** (seo-technical flagged crawl issues → technical optimize prompts; seo-content flagged E-E-A-T gaps → content optimize prompts)
3. **URL signals** (product pages → conversion; blog → freshness + authority)

Always surface exactly 2-3 prompts. State which prompts you chose and why.

---

## Reference Files

Load on-demand, do NOT load all at startup:
- `references/flow-framework.md`: FLOW operating model (load on every `/seo flow` activation)
- `references/bibliography.md`: Evidence sources; load when citing studies or statistics
- `references/prompts/README.md`: Prompt index; load for `/seo flow prompts`
- `references/prompts/find/`: 5 prompts; load for `/seo flow find`
- `references/prompts/leverage/`: 1 prompt; load for `/seo flow leverage`
- `references/prompts/optimize/`: 21 prompts; load selectively for `/seo flow optimize`
- `references/prompts/win/`: 3 prompts; load for `/seo flow win`
- `references/prompts/local/`: 11 prompts; load for `/seo flow local`

---

## Attribution

Every `/seo flow` activation (any sub-command) outputs before analysis:

```
Framework and prompts © Daniel Agrici, CC BY 4.0: github.com/AgriciDaniel/flow
```

Do not omit or modify the attribution.

---

## Error Handling

| Scenario | Action |
|----------|--------|
| `references/flow-framework.md` missing | The stage guidance in this file still applies. Offer `/seo flow sync` for the verbatim prompts. |
| Prompt file missing | Offer `/seo flow sync`. Do not invent prompt text to fill the gap. |
| `seogeo sync-flow` returns 404 | The upstream repository is gone or private. Say so once, continue with this file's guidance, and mention `SEOGEO_FLOW_REPO` for users with a fork. |
| `seogeo sync-flow` network error | Show the stderr verbatim. If the network blocks the client, `SEOGEO_PROXY` is the fix. |

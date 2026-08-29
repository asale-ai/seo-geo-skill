---
name: geo-visibility
description: Measure whether the answer engines actually mention a brand. Asks ChatGPT, Claude, Perplexity, Gemini, or a local Ollama model the questions real buyers type, then reports mention rate, share of voice against named competitors, which domains get cited instead, and how all of it moved since the last run. Use this whenever someone asks "does ChatGPT mention us", "are we losing AI visibility", "who gets cited instead of us", or wants AI-search visibility tracked over time.
license: MIT
allowed-tools:
  - Read
  - Grep
  - Glob
  - Bash
  - WebFetch
  - Write
---

# Answer-Engine Visibility

## What this measures, and what it does not

Every other GEO check in this toolkit inspects a page and predicts. This one
asks the engines and records the answer. Keep the difference visible in every
report you write, because it is the difference between advice and evidence.

Two kinds of provider, and they answer different questions:

| Provider | Env var | Answers from | Tells you |
|---|---|---|---|
| `perplexity` | `PERPLEXITY_API_KEY` | live retrieval, reports its sources | whether your **pages** get retrieved and cited |
| `openai` | `OPENAI_API_KEY` | model weights | whether your **brand** is known |
| `anthropic` | `ANTHROPIC_API_KEY` | model weights | whether your **brand** is known |
| `gemini` | `GEMINI_API_KEY` | model weights | whether your **brand** is known |
| `ollama` | none — runs locally | local model weights | free smoke-testing, no API bill |

Only retrieval providers can tell you a page was cited. If the user has no
retrieval key, say so in the report rather than presenting a mention rate as
a citation rate.

Check what is available before promising anything:

```bash
seogeo visibility providers
```

## The workflow

### 1. Build the prompt set — this is the measurement instrument

The prompt set decides the result. A set of branded questions will show a
flattering mention rate and tell the user nothing; a set of category questions
("best CRM for a two-person agency") is where visibility is actually won or
lost.

Get a scaffold, then **rewrite it**. You are a language model reading the
user's site — you will write better prompts than the template.

```bash
seogeo visibility prompts example.com --topic "project management software" --count 20
```

Save the rewritten set as JSON or one prompt per line:

```json
{"prompts": ["What is the best project management tool for a design studio?", "..."]}
```

Aim for 20–50 prompts, mostly unbranded, spread across the buying journey:
category discovery, comparison, objection ("what are the downsides of…"),
and use-case fit. Include 3–5 that name competitors but not the brand.

### 2. Cost the run before spending anything

Probes = prompts × providers. Fifty prompts against four engines is 200 API
calls. Always dry-run first and show the user the number.

```bash
seogeo visibility run --brand "Acme" --prompts prompts.json --dry-run
```

### 3. Run it

```bash
seogeo visibility run --brand "Acme" --domain acme.com --prompts prompts.json \
  --competitor "Globex" --competitor "Initech" --label "2026-Q3" --json
```

- `--domain` is what makes `cited_own_domain` work — without it, self-citations cannot be detected.
- `--competitor` is what makes share of voice meaningful. Without competitors, SoV is always 100%.
- `--alias` catches the other names people use for the brand ("Acme Corp", "AcmeHQ").
- `--label` shows up in the history table; use a release tag or a quarter.
- `--concurrency` defaults to 4. Raise it for large sets, lower it if a provider rate-limits.

### 4. Read the result

The fields that matter, in the order a client cares about them:

- **`mention_rate`** — of the answers that came back, how many named the brand.
- **`share_of_voice`** — brand mentions ÷ (brand + competitor mentions). This is the number that moves when a competitor invests and you do not.
- **`avg_prominence`** — 1.0 means named first, 0.1 means named as an afterthought. Engines list the best-known option first, so a falling prominence is an early warning that precedes a falling mention rate.
- **`citation_rate`** — of retrieval answers, how many cited the brand's own domain.
- **`cited_domains`** — who *is* getting cited. This is the most actionable list in the whole report: it is the set of pages you need to appear on, be reviewed on, or out-rank.
- **`unanswered_prompts`** — the exact questions where the brand is invisible. Hand these to the content plan.

### 5. Track it

One run is a snapshot and worth little. The value is the trend.

```bash
seogeo visibility history --brand "Acme" --days 90
seogeo visibility diff --brand "Acme" --days 30
seogeo visibility citations --brand "Acme" --days 90
```

`visibility diff` exits **2** when mention rate or share of voice fell, so it
can gate a scheduled job without parsing the report.

## One-off questions

For a single question, without recording a run:

```bash
seogeo ask "What is the best project management tool for a design studio?" --brand "Acme" --json
seogeo ask "Who are the leading CRM vendors?" --provider perplexity
```

Use this while iterating on the prompt set, and to reproduce a single
surprising result from a run.

## Validating the citability score

The toolkit's citability score claims to predict citations. On any site with a
retrieval key, that claim can be checked rather than asserted:

```bash
seogeo citability-validate example.com --prompts prompts.json --max-pages 30 --json
```

It scores the site's pages, asks the retrieval engines the prompt set, and
reports the rank correlation between a page's citability score and how often
it was actually cited, plus the mean score of cited versus uncited pages.

Report the `verdict.caveat` verbatim. Small samples produce confident-looking
correlations that mean nothing, and the command flags that itself — do not
strip the flag when summarising.

## Scheduling

```bash
seogeo watch --brand "Acme" --domain acme.com --prompts prompts.json \
  --competitor "Globex" --url https://acme.com/pricing --html
```

Writes dated JSON artifacts plus `report.html` under `seogeo-watch/`, and exits
**2** on a regression, **1** if a check could not run. Put it on cron or in a
scheduled CI job; nothing here needs a server.

A ready GitHub Actions workflow ships at `templates/geo-watch.workflow.yml` in
this repository — it runs weekly, caches the visibility history between runs so
the trend survives, uploads the report as an artifact, and opens an issue only
on exit 2. Copy it into the tracked site's repo rather than pointing it at this
one.

Weekly, not daily. Answer engines are noisy enough that a daily series is
mostly variance.

## Reporting

```bash
seogeo report-html --input visibility.json --output visibility.html --title "Acme"
```

One self-contained HTML file — no external CSS, fonts, or scripts — so it can
be emailed, committed, or dropped on a static host.

## Rules

- **Never present a model-knowledge answer as a citation.** If only `openai` and `anthropic` are configured, the report says the brand is known or unknown, not that pages were cited.
- **Always state the sample.** "42% mention rate" without "over 24 prompts × 2 engines" is not a finding.
- **Show the cost.** Every run reports `cost_usd`. Include it.
- **Never invent an answer.** If a probe failed, the report says it failed. Failed probes are excluded from rates, and the count is in `summary.failed`.
- **Rerun before concluding a drop is real.** Engines are non-deterministic; a single run's mention rate moves several points on its own. Two runs agreeing is the minimum evidence for a trend claim.

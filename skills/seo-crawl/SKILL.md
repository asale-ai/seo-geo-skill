---
name: seo-crawl
description: Crawl an entire website and find the problems that only appear across pages — duplicate titles and meta descriptions, broken internal links with the page that links to them, thin content, missing structured data, noindex pages that should not be, and heading gaps. Use this for a full site audit, a pre-launch check, a migration verification, or whenever a single-page audit is not enough.
license: MIT
allowed-tools:
  - Read
  - Grep
  - Glob
  - Bash
  - Write
---

# Site-Wide Crawl

## When to reach for this

Page-level commands answer "is this URL healthy?". Some failures are invisible
at that altitude:

- The same `<title>` on 400 product variants — each page looks fine alone.
- A 404 that only one page links to, and only from the footer.
- A section that scores well and that nothing links to.
- Thin content that is only obviously thin next to the rest of the site.

Use `seogeo parse` for one page. Use this when the question is about the site.

## Running it

```bash
seogeo crawl https://example.com --max-pages 100 --json
seogeo crawl https://example.com --max-pages 500 --max-depth 4 --concurrency 8
```

Defaults are the polite ones, and they are the defaults on purpose — this
ships inside an agent skill and will be pointed at sites the user does not own:

- **same host only.** `--include-subdomains` to follow into `blog.example.com`.
- **robots.txt honoured**, including `Disallow` under `*`. `--ignore-robots` exists for sites the user owns; confirm ownership before using it, and say in the report that it was used.
- **hard page budget.** `--max-pages` defaults to 100. Raise it deliberately.
- **`--delay-ms`** pauses between requests per worker. Use it on small or obviously fragile hosts.

Start at 100 pages to see the shape of the site, then decide whether a full
crawl is worth the time.

## Reading the output

`summary.issues` is ordered by severity and each entry carries a `why`.

**Critical**
- `broken-internal-links` — each entry names `linked_from`, so the fix has an address. This is the only issue here that costs crawl budget on every future crawl.
- `missing-title` — no snippet, and nothing for an answer engine to attribute a quote to.

**Warning**
- `duplicate-titles` / `duplicate-meta-descriptions` — usually one template that never got per-page values. Fix the template, not the pages.
- `thin-content` — under 300 words. Cross-reference with `geo-citability`: a thin page cannot contain a self-contained answer worth quoting.
- `no-h1` — the strongest single hint about what a page answers.

**Info** — `multiple-h1`, `noindex`, `no-structured-data`, `no-canonical`, `images-missing-alt`. Real, but rarely the reason a site underperforms. Do not lead with them.

The per-page array is in `pages` if you need to slice it yourself. Exits **2**
when a critical issue is present.

## Chaining

The crawl is the front of a pipeline, not the whole audit:

```bash
seogeo crawl https://example.com --max-pages 200 --json > crawl.json
seogeo citability https://example.com/the-worst-scoring-page
seogeo logs /var/log/nginx/access.log --sitemap example.com --json
seogeo report-html --input crawl.json --output audit.html
```

A useful sequence: crawl to find the thin and duplicate pages, score the worst
of them for citability, then check the logs to see whether the AI crawlers ever
fetched them at all. A thin page nobody crawls is a lower priority than a good
page that gets 403s.

## Rules

- **Report the budget.** "No broken links found" after crawling 100 of 8,000 pages is misleading. Always state `crawled` against the real site size.
- **Do not raise concurrency past 8** on a site the user does not own.
- **Say when robots was ignored.** `--ignore-robots` changes what the numbers mean.

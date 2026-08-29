---
name: geo-crawler-logs
description: Analyse server access logs to find out what the AI crawlers actually did — which bots arrived, what they took, what they were refused with 403 or 429, which pages they have never fetched, and how many human visits each AI platform sent back. Use this when someone asks whether ChatGPT or Perplexity can really reach their site, why AI traffic is not converting, what the crawl-to-referral ratio is, or wants AI bot traffic measured from first-party data rather than guessed.
license: MIT
allowed-tools:
  - Read
  - Grep
  - Glob
  - Bash
  - Write
---

# AI Crawler Log Analysis

## Why logs and not a probe

`seogeo robots` asks whether a crawler is *allowed*. Logs show whether it
*came*, what it *got*, and whether anyone *followed*. Those are three different
failures and only the last one is visible from outside:

- robots.txt allows GPTBot, but the WAF returns 403 to it → the audit passes, the site is invisible.
- The crawler fetches happily, but only the blog, never the product pages → coverage gap no external check can see.
- Crawlers take thousands of pages and send back no visits → an extraction relationship, not a distribution one.

This runs entirely on the operator's machine. Access logs contain visitor IP
addresses; nothing here is uploaded, and that is the reason this check belongs
in a local binary rather than a hosted dashboard. Say so when a user hesitates
to share logs.

## Running it

```bash
seogeo logs /var/log/nginx/access.log --json
seogeo logs /var/log/nginx/access.log.1 /var/log/nginx/access.log.2.gz --since 2026-06-01
cat access.log | seogeo logs -
```

`.gz` is read directly — point it at the whole rotated set. Formats
understood: NCSA combined/common, and JSON lines from Cloudflare Logpush,
Vercel, and structured nginx.

Add the site to find pages no AI crawler has ever successfully fetched:

```bash
seogeo logs /var/log/nginx/access.log --sitemap example.com --json
```

## The distinction the whole report turns on

Crawlers are classified by **purpose**, and the purposes are not
interchangeable:

| Purpose | Bots | Can it produce a citation? |
|---|---|---|
| `search` | OAI-SearchBot, Claude-SearchBot, PerplexityBot, Googlebot, bingbot, DuckAssistBot | **Yes** — it builds the index an answer cites |
| `user-action` | ChatGPT-User, Claude-User, Perplexity-User, MistralAI-User | **Yes, and right now** — a person in an assistant asked for this page |
| `training` | GPTBot, ClaudeBot, CCBot, Bytespider, Google-Extended, meta-externalagent | **No** — corpus collection only |
| `other` | GoogleOther, Amazonbot | unclear |

A site with 40,000 training crawls and zero search crawls has given away its
content and received nothing. That is a finding, and it is the first thing to
say. Never report a single combined "AI bot traffic" number — it hides exactly
the part that matters.

## Reading the output

- **`by_purpose`** — the split above. Lead with it.
- **`platforms[].crawl_to_referral`** — pages taken per human visit returned. Ratios in the thousands are normal in 2026 and still worth stating plainly; the point is not outrage, it is that crawl volume is not a success metric.
- **`bots[].blocked_403` / `rate_limited_429`** — **check this first**. A search-purpose bot being refused is a critical finding: permission was granted and delivery failed. The cause is almost always bot management or a WAF rule, not robots.txt, so `seogeo robots` will report everything as fine.
- **`referrals`** — humans arriving from an assistant, detected by referrer. Zero referrals with heavy crawling means either you are not being cited, or the referrer is being stripped by a CDN or redirect hop. Check the second before concluding the first.
- **`coverage`** — sitemap URLs no AI crawler has fetched. These pages cannot be cited by anyone, no matter how well written.
- **`findings`** — pre-computed, with a `why` on each. Use them; do not re-derive.

`other_requests` counts every user-agent the tool does not recognise as an AI
crawler — humans *and* ordinary bots. It is not a human-visitor count, so do
not report it as one.

The command exits **2** when a critical finding is present, so it can gate a
scheduled job.

## What to recommend

Match the fix to the finding, not to a checklist:

- **Search bots getting 403/429** → the WAF or bot-management rule. Allowlist the search-purpose user agents by verified IP range. This is the highest-value fix available and it is usually one config change.
- **Training crawls only** → the site is reachable but not in any answer index. Check `llms.txt`, sitemap freshness, and whether the pages are server-rendered.
- **Good crawling, no referrals** → retrieval works, citation does not. That is a content problem, and it hands off to `geo-citability` and `geo-visibility`.
- **Coverage gaps** → internal linking and sitemap priority, not content.

## Reporting

```bash
seogeo logs /var/log/nginx/access.log --json > logs.json
seogeo report-html --input logs.json --output crawlers.html
```

# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every release ships prebuilt archives for macOS (Intel and Apple Silicon), Linux (gnu and musl,
x86_64 and aarch64) and Windows, together with a `SHA256SUMS` file. Verify the checksum before
running a downloaded binary.

## [Unreleased]

## [0.2.0] - 2026-08-29

### Added

- **Answer-engine measurement.** `seogeo ask` and `seogeo visibility` put real questions to
  Perplexity Sonar, OpenAI, Anthropic, Gemini, or a local Ollama model and record whether the
  brand was named, how prominently, which competitors appeared alongside it, and which domains
  were cited. Keys are the caller's own; nothing is proxied. Runs are stored in SQLite so
  `visibility history`, `visibility diff`, and `visibility citations` can report the trend —
  `diff` exits 2 when mention rate or share of voice falls.
- **AI crawler log analysis.** `seogeo logs` parses combined, common, gzipped, Cloudflare
  Logpush, and Vercel access logs and reports what the AI crawlers did: which arrived, what
  they were refused with 403/429, the crawl-to-referral ratio per platform, and — with
  `--sitemap` — which pages no AI crawler has ever fetched. Crawlers are separated by purpose,
  because a training crawl can never produce a citation and a search crawl can.
- **Site-wide crawl.** `seogeo crawl` walks a site within a page budget, honouring robots.txt
  and staying on-host by default, and aggregates the failures that are invisible one page at a
  time: duplicate titles and descriptions, broken internal links with the page that links to
  them, thin content, and missing structured data.
- **MCP server.** `seogeo mcp` serves 14 tools over stdio JSON-RPC, reaching hosts that do not
  read `SKILL.md` files.
- **`seogeo citability-validate`** checks the citability score against reality: it scores a
  site's pages, asks the retrieval engines a prompt set, and reports the rank correlation
  between score and actual citations — flagging when the sample is too small to mean anything.
- **`seogeo report-html`** renders any `--json` output as one self-contained HTML file, and
  **`seogeo watch`** runs the scheduled checks, writes dated artifacts, and exits 2 on a
  regression. A GitHub Actions template ships at `templates/geo-watch.workflow.yml`.
- Three skills for the above: `geo-visibility`, `geo-crawler-logs`, `seo-crawl`.

### Fixed

- Requests to a local endpoint no longer go through `HTTPS_PROXY`, which answered 502 for
  127.0.0.1 and made a working local model server look broken.

## [0.1.4] - 2026-08-14

### Added

- Continuous integration on every push and pull request: rustfmt, clippy with warnings denied,
  `cargo build` and `cargo test` on Linux, macOS and Windows, and a RustSec advisory check.
- Dependabot updates for cargo dependencies and GitHub Actions.
- Code of conduct, bug report and feature request issue forms, an issue chooser that points
  security reports to private advisories, and a pull request checklist.
- This changelog.

### Fixed

- `content-verify` no longer reads statistics out of markup ([#8]). CSS percentages (`width:100%`,
  `style="…"` attributes), percent-encoded URLs (`?category=%E5%93%81` yielding `93%`), inline
  scripts and HTML comments are masked before claims are scanned, so a page whose body text
  contains no numbers reports no claims. Citations are still read from the original markup,
  so `<a href="https://…">` next to a claim keeps counting as a source.

## [0.1.3] - 2026-08-12

### Added

- Distribution of the skills through `npx skills`.

### Changed

- Bumped the GitHub Actions used by the release pipeline.

## [0.1.2] - 2026-08-12

### Fixed

- The released binary now matches the published skills.
- Documented flags match the implemented flags, every skill declares MIT, and a budget gate
  guards the commands that can spend API quota.
- The ClawHub publish step is idempotent and its verification reports honestly.
- Installation falls back to a usable location when no agent tool is detected.

## [0.1.1] - 2026-08-12

### Added

- First public release: 48 SEO and GEO skills, 23 subagents, and the `seogeo` binary that
  executes them, with a verified multi-platform release pipeline.

[Unreleased]: https://github.com/asale-ai/seo-geo-skill/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/asale-ai/seo-geo-skill/releases/tag/v0.1.1
[#8]: https://github.com/asale-ai/seo-geo-skill/issues/8

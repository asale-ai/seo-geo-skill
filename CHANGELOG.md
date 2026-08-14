# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every release ships prebuilt archives for macOS (Intel and Apple Silicon), Linux (gnu and musl,
x86_64 and aarch64) and Windows, together with a `SHA256SUMS` file. Verify the checksum before
running a downloaded binary.

## [Unreleased]

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

[Unreleased]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/asale-ai/seo-geo-skill/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/asale-ai/seo-geo-skill/releases/tag/v0.1.1
[#8]: https://github.com/asale-ai/seo-geo-skill/issues/8

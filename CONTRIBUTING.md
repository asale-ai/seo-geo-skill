<!-- SPDX-License-Identifier: MIT -->

# Contributing

## Layout

```
src/                Rust sources for the seogeo binary
  safety.rs         SSRF policy — every request goes through it
  http.rs           HTTP client, charset decoding, proxy support
  html.rs           HTML → SEO elements, content blocks, visible text
  chrome.rs         Headless Chrome bridge (render, screenshot, PDF)
  paths.rs          Where config, data, and cache live
  cli.rs            The clap command surface
  cmd/              One module per command family
skills/             48 skills, embedded into the binary at build time
agents/             23 subagent definitions, installed alongside the skills
schema/             JSON-LD templates the schema skills reference
templates/          Report stylesheets
data/               Bundled datasets (the Google update timeline)
tests/              Skill ↔ CLI parity gate
```

`skills/` and `agents/` are embedded with `include_dir!`, so a downloaded
binary can install them without a checkout. Adding a skill directory is enough
to ship it — no manifest to update.

The flat `skills/<name>/SKILL.md` layout is also exactly what
[`npx skills`](https://github.com/vercel-labs/skills) expects, which is how
`seogeo install --target npx` reaches 75+ agents. Keep the layout flat: a
nested one changes how that tool discovers skills.

## Build

```bash
cargo build            # debug
cargo build --release  # optimised, ~5.5 MB stripped
cargo test             # unit tests + the parity gate
cargo clippy --all-targets
cargo fmt
```

Requires Rust 1.82 or newer. The release profile uses `opt-level = "z"`, fat
LTO, one codegen unit, `panic = "abort"`, and symbol stripping — the binary is
downloaded by every user, so size is a feature.

## The parity gate

`tests/skill_cli_parity.rs` is the test that will fail on you first, and that
is deliberate. It enforces:

1. Every `seogeo <command>` mentioned in a skill or agent resolves to a real
   subcommand. A typo here ships a skill that fails in front of a user.
2. Every subcommand is documented by at least one skill. Operator-only
   commands (`install`, `commands`, `sync-flow`) are exempt.
3. `seogeo commands` covers the CLI exactly — no gaps, no stale entries.
4. Every skill has well-formed frontmatter whose `name` matches its directory.
5. No skill invokes Python. The whole point of this repo is that it does not.

So adding a subcommand means three edits, not one:

- the `Command` enum in `src/cli.rs` and its dispatch arm in `src/cmd/mod.rs`
- an entry in `COMMAND_MAP` in `src/cmd/misc.rs`
- a documented invocation in the skill that uses it

## Adding a skill

```
skills/<skill-name>/
  SKILL.md          required; frontmatter `name` must equal <skill-name>
  references/       optional, loaded on demand by the agent
  templates/        optional
```

Frontmatter needs at least `name` and `description`. The description is the
only thing an agent sees before deciding whether to load the skill, so write it
as trigger phrases a user would actually say — not as a summary of the file.

Keep `SKILL.md` under ~500 lines. Anything longer belongs in `references/`,
which the agent reads only when it needs to.

## Conventions

- Every command supports `--json`. Skills parse JSON; the human-readable form
  is lossy on purpose.
- JSON goes to stdout, everything else to stderr. A skill piping stdout into a
  parser must never get a progress message mixed in.
- Exit codes carry meaning: `0` pass, `1` the check failed or a threshold was
  missed, `2` bad input or a blocked URL.
- Never fetch a user-supplied URL outside `http::get` / `http::post_json`.
  There is no legitimate reason to bypass the safety layer, and a review will
  reject it.
- Error messages name the missing thing and what it unlocks. "Google API key
  not configured. Set GOOGLE_API_KEY" is useful; "authentication failed" is
  not.
- Redact credentials before printing anything. `google::redact` exists because
  agents log stderr and those logs get pasted into issues.

## Testing against real sites

Unit tests use fixtures. Behavioural changes should also be checked against a
live page, because real HTML is stranger than any fixture:

```bash
cargo run -- fetch https://example.com --json | head -40
cargo run -- citability https://developer.mozilla.org/en-US/docs/Web/HTTP/Caching
cargo run -- robots https://developer.mozilla.org
```

Some networks interfere with this client's TLS handshake for particular hosts,
which looks like the site being down. If `curl` works and `seogeo` does not,
set `SEOGEO_PROXY` before concluding you found a bug.

## Releasing

Tag-triggered. `.github/workflows/release.yml` cross-compiles macOS
(arm64 + x64), Linux (x64 + arm64, gnu and musl), and Windows x64, then uploads
the archives plus `SHA256SUMS` to the GitHub release.

`./publish.sh "commit message"` does the whole thing unattended: bumps the
patch version, updates `Cargo.lock`, commits, pushes, tags, and pushes the tag.
Pass `--minor`, `--major`, or `--version X.Y.Z` to control the bump.

## Pull requests

- One concern per PR.
- `cargo test` and `cargo clippy` clean.
- If you change behaviour a skill documents, update that skill in the same PR —
  the parity gate checks the wiring, but only you can check that the prose
  still describes what the code does.
- New dependencies need a reason in the PR description. Every crate is weight
  in a binary users download.

## Licence

Contributions are MIT, matching the project. Material derived from other
sources must be recorded in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)
with its licence.

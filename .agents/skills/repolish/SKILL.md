---
name: repolish
description: >-
  Diagnose and improve what an open-source repository looks like to a first-time
  visitor. Use when asked to review, score, or improve a README, a repository's
  documentation, its "first impression", its discoverability, or its
  open-source hygiene (license, CI, contributing guide, issue templates) — and
  before rewriting any README by hand. Also use to generate the SVG overview
  card, the report card, SVG tables, or a VHS terminal recording for a CLI.
---

# repolish

A command-line tool that scores 22 concrete signals about a repository and, for
every point it deducts, names the file and line and says what to write instead.
Then it applies the fixes that follow mechanically — **by editing the author's
`README.md` in place, insert-only** — and draws the SVGs the README refers to.

**No model is in the scoring path.** The same commit always produces the same
score. You may suggest wording; you must never claim a number the tool did not
produce.

## The rule that matters most

**Do not rewrite the README.** Your instinct will be to replace it with a
well-structured one. That destroys the author's voice, their layout, their
examples, and usually their accuracy. Instead:

1. **Measure.** `repolish` — one command, no arguments. It scores the repository
   and prints every file it would touch, and it writes **nothing**.
2. **Show the user that plan, then apply it.** `repolish --apply` writes it.
3. **Hand back what needs judgement.** A missing quickstart, a vague tagline, a
   README claim whose command no longer exists — those need the author's
   knowledge, or a targeted edit you can justify from evidence.
4. **Measure again** and report the delta — `repolish --stages check --base <ref>`
   does the arithmetic for you and lists only the checks that moved.

Only edit prose directly when a finding names a specific line and the fix is
unambiguous, and say which finding you were acting on.

## Which invocation to use

**Check first, then install.** Every command below calls `repolish` by name, so
start by finding out whether it is already there:

```bash
repolish --version
```

If that prints a version, use `repolish` as written throughout this document. If
it says "command not found", reach for npx — it needs nothing installed and
works wherever Node does:

```bash
npx -y @asale/repolish
```

Then read every `repolish …` below as `npx -y @asale/repolish …`, **for the rest
of the session**. The `-y` matters: without it npx stops to ask, and you are not
at a terminal to answer.

**npx does not install anything onto PATH.** It downloads into a cache and runs
from there, so `repolish` still will not exist after you have used it once — and
the tool's own output says `npx @asale/repolish …` back to you for exactly that
reason. Never drop the prefix partway through, and never tell the user to run
bare `repolish` unless `repolish --version` worked above.

That package is a launcher — it downloads the release binary, verifies its
`.sha256` and execs it, forwarding the exit code, which is what `--min-score`
depends on. The current version is 0.4.3.

If the user would rather have it on PATH permanently:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/repolish/main/install.sh | sh
```

`cargo install repolish` works too, as does a release binary from
`https://github.com/asale-ai/repolish/releases` (five targets, each with a
`.sha256`). Linux builds are glibc-only; on musl, use cargo.

## What the bare command actually does

**There are no subcommands.** `repolish [PATH]` is the whole surface — `PATH`
defaults to `.` — and `--stages` picks which parts of the pipeline run. The
default, when you pass no `--stages`, is:

```text
check → polish → artifacts → ci → demo
```

The order is load-bearing: `polish` inserts a reference to a card, and
`artifacts` is what redraws it on every run after that.

| Stage | What it does | In the default run? |
|---|---|---|
| `check` | Score the repository and print the report | yes |
| `polish` | The mechanical fixes — **edits `README.md` in place** and writes four new files | yes |
| `artifacts` | `.repolish/badge.json`, and redraws every SVG the README already references | yes |
| `ci` | `.github/workflows/repolish.yml` | yes |
| `demo` | Record the CLI as an animated SVG and reference it from the README. **`--apply` RUNS the commands** | yes |
| `skill` | `SKILL.md`, or install this skill into an agent's own directory | **no — opt-in** |

**Nothing is written without `--apply`.** Bare `repolish` prints the score, then
every line it would insert and every file it would create, and stops. That dry
run is the entire safety story. Two exceptions, and only two: `--sarif PATH` and
`--comment PATH` write the path you named, because naming a path is itself the
request.

A run that skipped `demo` or `skill` says so at the end, under `NOT RUN`.

### `--apply` edits `README.md` itself

This is the part to be precise about with the user: `polish` does not emit a
patch file or a suggested README beside the original. **It rewrites
`README.md` in place**, and every translation next to it (`README.zh-CN.md`,
`README.ja.md`, …) the same way.

It is **insert-only**. Not a single existing line is edited, reordered or
deleted, so tabs, list markers, reference-style link definitions and line
endings survive byte for byte. The diff is new lines and nothing else.

What it inserts, in the order it appears in the file:

| Insertion | Where | Conditions |
|---|---|---|
| Project banner (`.repolish/hero.svg`) | above the title | on by default; skipped if `--logo` was given, or an image already sits above the title |
| Your own logo | above the title | only with `--logo <path-in-repo>` |
| The repolish badge | with the other badges | when the badge is not already there |
| Overview card (`.repolish/overview.svg`) | under the badges | on by default |
| Table of contents | after the intro | built from the author's **own** headings |
| SVG tables | in place of each markdown table, original folded into `<details>` | on by default |
| Project tree | appended | **only** if `--tree-depth N` or the config asks — no check requires it |
| Report card (`.repolish/card.svg`) | at the very end, under its own heading | on by default |

The banner, the two cards and the table SVGs are the **visuals**, and they are
**on by default**. `--no-visuals` leaves the README's visuals alone — reach for
it when the author has clearly art-directed the file themselves. The single
switches (`--hero`, `--overview`, `--footer-card`, `--tables svg`) win over
`--no-visuals`, so `--no-visuals --overview` means "that one only".

Do not swap the two cards. The overview card belongs at the top because the top
of a README belongs to the project; the report card belongs at the bottom
because a visitor should not meet our tool grading the project before meeting
the project.

### and it creates these new files

`.github/ISSUE_TEMPLATE/bug_report.yml`, `.github/ISSUE_TEMPLATE/feature_request.yml`,
`.github/pull_request_template.md`, and a `CONTRIBUTING.md` whose build and test
commands come from the **detected package manifest**.

**Where it cannot know, it does not write.** No manifest means no
`CONTRIBUTING.md`, because the alternative is `<your build command here>` — a
file that turns the check green while the problem stays exactly where it was.
It will not write a code of conduct at all, and it will not pick a licence.

Files that already exist are left alone. `--force` regenerates them, and also
lets `--apply` run outside a git repository — which it otherwise refuses to do,
because `git checkout` is the undo button. **Never pass `--force` on the user's
behalf without saying so.**

### and `demo` runs real commands

The `demo` stage is in the default run, but without `--apply` it only prints the
list of commands it would execute. With `--apply` it **runs them for real**,
records the terminal, writes `.repolish/demo.svg`, and inserts the reference
into the README and every translation.

That is the point — the output in the recording is real — but it means the dry
run is also the consent. Show the user the command list before you pass
`--apply`, and never pass `--cmd` with anything destructive.

**Its default records only `<binary> --help`, which is almost never the demo the
project deserves.** Picking the commands that show the project doing its actual
work is your call, not the tool's — see
[the demo stage](#the-demo-stage--and-why-you-must-pass---cmd) before you run it.

### Report the aftermath honestly

`--apply` prints `WROTE (N files)`. Tell the user how to see it and how to undo
it, because `git diff` alone hides the new files:

```bash
git add -A && git diff --staged
```

Undo is `git checkout -- . && git clean -fd`.

Anything skipped for want of an input is listed at the end under `NEEDS INPUT`,
each with the command that fixes it. Relay those rather than silently accepting
a thinner result: a missing `GITHUB_TOKEN` is why three checks read as *not
verified*, and an unbuilt binary is why there is no recording.

## Task → command

| The user wants | Run |
|---|---|
| a score, nothing touched | `repolish --stages check` |
| a score you are going to act on | `repolish --stages check --format json` |
| "improve my repo" | `repolish` → show the plan → `repolish --apply` |
| the fixes but no badge JSON, no CI workflow, no recording | `repolish --stages check,polish --apply` |
| only the README edits, no new side files | not separable — read the plan and drop what they object to yourself |
| to leave the visuals alone | `repolish --apply --no-visuals` |
| a CI workflow | `repolish --stages ci --min-score 70 --apply` |
| the badge only | `repolish --stages artifacts --artifact badge --apply` |
| the cards redrawn | `repolish --stages artifacts --apply` |
| a terminal recording | pick the project's real commands, then `repolish --stages demo --cmd … --cmd …` → show the list → `--apply`. **Do not take the `--help` default** |
| to know what a PR did to the score | `repolish --stages check --base origin/main` |
| findings on the PR diff | `repolish --stages check --sarif repolish.sarif` |
| this skill installed for their agents | `npx skills add asale-ai/repolish` |
| `SKILL.md` committed into the repo | `repolish --stages skill --apply` |

## Every option, grouped

### Choosing what runs

| Flag | Meaning |
|---|---|
| `[PATH]` | the repository; defaults to `.` |
| `--stages <list>` | comma-separated: `check,polish,artifacts,ci,skill,demo` |
| `--apply` | write. Without it, nothing lands |
| `--force` | overwrite existing files, and allow `--apply` outside a git repository |
| `--only <ids>` / `--skip <ids>` | run just these checks / everything but these |
| `--profile <p>` | override the detected type: `library`, `app`, `cli`, `docs`, `collection`, `meta`, `unknown` |
| `--config <path>` | defaults to `.repolish.toml` in the repository root |
| `-v` | P3 suggestions, passing checks, and the full contents of every new file |

### Talking to GitHub

**The GitHub API is called by default whenever `GITHUB_TOKEN` or `GH_TOKEN` is
set.** That is not what an older version of this document said; do not add
`--remote` reflexively.

| Flag | Meaning |
|---|---|
| *(nothing)* | remote if a token is in the environment, local if not — and it says which |
| `--remote` | force it, anonymously too, on a quota of 60 requests per hour |
| `--no-remote` | never call. `repo-description`, `repo-topics`, `repo-homepage` report as *not verified* |
| `--stars` / `--no-stars` | the star-history curve on the overview card, about a dozen extra API calls |

To hand the tool a token: `export GITHUB_TOKEN=$(gh auth token)`.

### Reporting

| Flag | Meaning |
|---|---|
| `--format text\|json\|markdown\|sarif\|comment` | `text` is the default |
| `--min-score <n>` | exit 1 below it, and gate the generated CI workflow on it |
| `--no-gate` | generate a workflow that records the score without enforcing it |
| `--base <ref>` | also score that ref and report only what moved. A temporary worktree; your tree is untouched |
| `--sarif <path>` | SARIF 2.1.0. **Written even without `--apply`** |
| `--comment <path>` | the short PR-comment form. **Written even without `--apply`** |
| `--report` | also write `REPOLISH.md`, the full report as markdown |

In every format but `text`, **stdout carries only the report** and everything
procedural goes to stderr — so `repolish --format json | jq` works on a full
pipeline run, not just on `--stages check`.

### The artifacts stage

```bash
repolish --stages artifacts --apply                     # badge + redraw what the README references
repolish --stages artifacts --apply --artifact overview # just .repolish/overview.svg
repolish --stages artifacts --apply --artifact score    # just .repolish/card.svg
repolish --stages artifacts --apply --theme porcelain   # light palette, for a light README
repolish --stages artifacts --apply --lang zh-CN        # en / zh-CN / ja; follows the README by default
repolish --stages artifacts --apply --remote --stars    # with the star history curve
```

Without `--artifact` this stage redraws **only what is already there**: the
badge JSON, plus `hero.svg` / `overview.svg` / `card.svg` if those files exist,
plus the SVG for every README table already wrapped in `<details>`. Naming one
with `--artifact` drops that requirement — the naming *is* the requirement.
`--no-badge` skips the badge JSON.

`--artifact` takes `badge`, `hero`, `report`, `overview`, `score`, `tables`.
`-o <path>` and `--stdout` work only when exactly one stage is selected and, in
this stage, exactly one `--artifact`.

Every SVG is self-contained and deterministic: no external fonts, no scripts,
nothing hosted by a third party, and the same commit renders a byte-identical
file.

### How the insertions look

None of these move a score. `--badge-style` (`flat`, `flat-square`, `plastic`,
`for-the-badge`, `social`), `--align` (`left`, `center`), `--toc-style`
(`bullet`, `number`, `roman`, `fold`), `--logo <path>`, `--logo-width <px|full>`,
`--tree-depth <n>`, `--theme` (`dark`, `porcelain`), `--lang`
(`auto`, `en`, `zh-CN`, `ja`), `--branch <name>` for the badge URL.

```bash
repolish --apply --logo assets/hero.svg --logo-width full --align center
```

### The demo stage — and why you must pass `--cmd`

```bash
repolish --stages demo                  # list what it would run, run nothing
repolish --stages demo --apply          # run them, and write .repolish/demo.svg
repolish --stages demo --apply --cmd "tool init" --cmd "tool build" --cmd "tool ship"
repolish --stages demo --apply --tape   # also write .repolish/demo.tape, for a GIF via VHS
repolish --stages demo --apply --type-ms 30
```

**The default is `<binary> --help`, and it is a floor, not a recommendation.**
`--help` is the only command that is true of *every* CLI, so it is all the tool
can safely guess on its own — one more guess at a subcommand and half the time
it records a screenful of `unknown subcommand`. A README whose only animation is
a help screen has shown the reader a wall of flags and nothing the project
actually does.

**Closing that gap is your job, and it is one of the clearest wins you have
here.** You have read the codebase; the tool has not. So: **do not run the demo
stage on its defaults. Choose the project's real commands and pass them with
`--cmd`.**

`--cmd` **replaces** the default entirely; it does not append to it. Repeat the
flag, once per command, in the order they should play.

#### Finding the right commands

Look in this order, and prefer what is already proven to work:

1. **The README's quickstart and usage sections.** If the author already decided
   which command a newcomer should type first, record that one — and you have
   just made the animation and the prose agree.
2. **The manifest.** `bin` in `package.json` or `Cargo.toml`, the npm `scripts`,
   the `Makefile` targets.
3. **`examples/`, and the integration tests.** These are commands someone has
   already checked actually run.
4. **The tool's own `--help`.** Run it, read the subcommand list, and pick the
   one or two that are the point of the project.

#### What to record

**The smallest real task, from start to finish** — not a feature tour. Three to
five commands. The strongest shape is an arc a reader can follow: a starting
state, the command that changes it, and the result.

repolish records its own README that way, and it is the example to copy:

```bash
repolish "$sample" --stages demo --apply \
  --cmd "repolish" \
  --cmd "repolish --apply" \
  --cmd "repolish" \
  --output .repolish/demo.svg
```

Score, fix, score again — the second number is visibly higher than the first,
and nothing about it is staged.

#### Rules the recording imposes

- **The commands really run.** Verify each one exits 0 *yourself* before you
  pass `--apply`. A command that fails is still recorded, with only a warning on
  stderr — a demo that quietly ships an error screen is worse than no demo.
- **There is no shell.** The string is split on whitespace (quotes respected)
  and executed directly. `|`, `>`, `&&`, `$(…)`, globs and `VAR=x` prefixes are
  passed through as literal arguments, and will appear in the recording as text.
  If you need a pipeline, record a script that contains it.
- **The binary must be on PATH.** Build first, then put the build directory on
  PATH for that one invocation — the failure message names the two commands for
  cargo, npm and go projects. Do not edit the user's shell configuration.
- **Nothing destructive, nothing slow, nothing that needs a secret.** These run
  on someone else's machine, and the output goes into a public README. No
  network calls that could hang, no `rm`, no anything that writes outside the
  repository.
- **Beware output that changes every run.** Commit hashes, timestamps, elapsed
  times and "N days ago" all churn the SVG on every re-record, and the diff will
  never settle. Prefer commands whose output is stable.
- **Commands share a working directory and run in sequence**, so state carries
  from one to the next. Terminal width is fixed at 100 columns.
- **Show the list, then apply.** Without `--apply` the stage only prints what it
  would execute — that print-out is the user's consent to running it. Never skip
  straight to `--apply` on a repository whose commands you have not read.

If no binary is detected the stage says so under `NEEDS INPUT` and moves on —
most repositories are not CLIs, and that is not an error. If the project *is* a
CLI and detection missed it, that is exactly the case `--cmd` is for.

The output is an animated SVG with a real, selectable text layer, not a GIF, and
it needs no external tools. `--tape` additionally writes a VHS script for
anyone who needs a GIF instead — npm and PyPI sanitise README HTML more
aggressively than GitHub does, so check the package page before committing to
SVG as the only format.

### The skill stage — installing this document

```bash
repolish --list                                   # which agents are installed on this machine
repolish --stages skill --apply                   # write SKILL.md into the repository
repolish --stages skill --target detect --apply   # install into the agents that are present
repolish --stages skill --target claude,codex --apply
repolish --stages skill --stdout                  # print it
```

`--target` takes `detect`, `all`, or any of `claude`, `codex`, `gemini`,
`opencode`, `agents`; it writes under the user's home directory, so the skill is
available in every project. Without `--target` it writes `SKILL.md` into the
repository, where it travels with the code. Either way it needs `--apply`, and
an existing file is left alone unless you pass `--force`.

For a user who just wants the skill and does not have repolish yet, the
ecosystem CLI is simpler and works for 70+ agents:

```bash
npx skills add asale-ai/repolish
```

### Wording — the one thing a model is for

```bash
repolish --suggest                  # needs REPOLISH_LLM_API_KEY, or ANTHROPIC_API_KEY
```

This asks a model for the three pieces no mechanical rule can write: the
tagline, the quick start, the usage example. It **prints and never writes**, not
even with `--apply`, and it does not move a score.

You usually do not need it — you are a model, and you are already here. It
exists for the author running the CLI without you. If you are drafting those
sections yourself, hold to the same three rules it does: fill the gap, never
rewrite what is there, and never invent a command that is not in the manifest.

## Configuration

Anything the user would otherwise repeat goes in `.repolish.toml` at the
repository root. The command line always wins, and an **unknown key is an
error**, not a silent no-op.

```toml
profile   = "cli"
min_score = 70

[checks]
only = []
skip = ["repo-topics"]

[readme]                 # keys are kebab-case; none of this moves a score
badge-style = "flat"
align       = "center"
toc-style   = "fold"
logo        = "assets/logo.svg"
logo-width  = "full"
tree-depth  = 2
theme       = "porcelain"
lang        = "auto"
hero        = true
overview    = true
footer-card = true
tables      = "svg"

[suggest]                # no API key here: this file is committed
model    = "claude-sonnet-4-5"
base-url = "https://api.anthropic.com"
```

Per-check thresholds are deliberately **not** configurable. Letting every
repository tune its own would make scores incomparable, which is the reason the
tool exists. Do not go looking for a way around that.

## Reading the JSON

**Prefer `--format json` when you are going to act on the result.** The text
output is laid out for a human reading a terminal; the JSON is stable, frozen at
`schemaVersion: 1`, and tells you per check the score, the evidence (file and
line) and the fixes.

```json
{
  "repolishVersion": "0.4.3",
  "schemaVersion": 1,
  "repository": { "owner": "acme", "name": "taskvault", "commit": "796160e…" },
  "profile": { "detected": "cli", "overridden": false },
  "mode": "local",
  "score": 82,
  "coverage": 0.811,
  "categories": [{ "category": "discoverability", "score": 100 }],
  "checks": [
    {
      "id": "readme-title-tagline",
      "category": "discoverability",
      "risk": "critical",
      "status": "scored",
      "score": 7,
      "evidence": [
        { "file": "README.md", "line": 1, "note": "the description is only 18 characters…" }
      ],
      "fixes": [
        { "severity": "P3", "message": "Expand the opening description past 20 characters…",
          "autofixable": false }
      ]
    },
    { "id": "repo-topics", "category": "discoverability", "risk": "high",
      "status": "skipped", "reason": "requires --remote" }
  ],
  "coverageLimits": ["repo-topics: requires --remote"]
}
```

Things that will bite you if you skim it:

- **`status` is a flat field on the check**, not a nested `outcome` object.
  It is one of `scored`, `not_applicable`, `inconclusive`, `skipped`.
  Only `scored` carries `score` / `evidence` / `fixes`; the other three carry
  `reason` (or `profile`) instead. **Never present them as passes** — they are
  excluded from the denominator on purpose.
- **`score: null` means "no score", not zero.** Fewer than half the registered
  weight could be scored. Report that honestly rather than picking a number.
- `risk` is `critical` / `high` / `medium` / `low` and weights 10 / 7.5 / 5 / 2.5.
- `severity` is `P1` / `P2` / `P3`.
- `mode` says `local` or `remote`, and it reflects **what actually happened**,
  not which flag was passed.
- `delta` appears **only** with `--base`, and lists only the checks that moved.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Score below `--min-score` |
| 2 | Bad arguments |
| 3 | Not a valid repository |
| 4 | `--remote` failed (API error, rate limit, private repo) |
| 5 | Less than half the total weight could be scored — no total score is reported |
| 7 | `--base` could not be resolved: shallow clone, unknown ref, no git |

Codes 4 and 7 are deliberately distinct from code 1. A rate limit and a shallow
clone are not quality regressions, and must never be reported as one. **Say which
happened**; do not summarise either of them as "the check failed". 6 is
permanently vacant — it used to mean something else, and reusing it would
mislead old scripts.

## The 22 checks

**Discoverability** — `readme-title-tagline`, `repo-description`†,
`repo-topics`†, `repo-homepage`†, `readme-badges`

**Comprehensibility** — `readme-quickstart`, `readme-usage-example`,
`readme-install-consistency`, `readme-link-health`, `readme-length`,
`docs-presence`, `readme-toc`, `readme-i18n`

**Credibility** — `license`, `claim-consistency`, `ci-present`, `tests-present`,
`activity`, `contributing`, `issue-pr-template`, `release-hygiene`,
`code-of-conduct`

† needs the GitHub API. Those three are the reason a local score and a remote
score are not comparable — say which one you ran.

## Making the calls repolish cannot

This is the part of the job that is actually yours, so it is worth being precise
about where the tool stops. repolish decides three different ways, and only two
of them are strong:

1. **Facts.** Does a LICENSE file exist, is there a workflow, how many headings
   are there. Filesystem and Markdown AST. Not arguable.
2. **Cross-references.** `claim-consistency` takes the commands out of the README
   and checks them against the manifest and the filesystem.
   `readme-install-consistency` checks that the install command installs *this*
   package. These are joins between two sources of truth, not opinions — and
   they are the checks worth acting on first.
3. **Graded heuristics.** `readme-quickstart` scores 0/4/6/8/10 from
   hand-curated substring lists. Most of the README checks have a list like that
   somewhere.

So the score is honestly a measure of **whether the machinery a reader needs is
present and whether the promises are true.** It is not a measure of whether the
writing is any good. repolish will happily give 10/10 to a quickstart made of
the right keywords in the wrong order.

That is the gap you are here to close. **Do not try to close it by rewriting.**
Work finding by finding:

| Finding | What a good fix looks like | The failure mode to avoid |
|---|---|---|
| `claim-consistency` | Make the claim true (restore the script, add the npm script) or correct the text to what actually works | **Deleting the line.** That turns the check green and leaves the reader with no instructions at all |
| `readme-title-tagline` | One line saying what it does and who it is for, in the author's register | Replacing a specific tagline with "A blazingly fast, modern toolkit for…" |
| `readme-quickstart` | The shortest path from zero to one working result, with the prerequisite named | Turning it into a feature tour, or inventing a command you have not run |
| `readme-usage-example` | A real example lifted from tests or `examples/` | Inventing an API that does not exist. Check it compiles or runs |
| `readme-length` | Move reference detail into `docs/` and link it | Deleting the detail |
| `license` | Tell the author their options and let them choose | Picking one for them. It is a legal decision, not a formatting one |
| `code-of-conduct` | Ask for a real reporting address | A Contributor Covenant with a placeholder email promises a channel that does not exist |
| `contributing` | Let `polish` take build and test commands from the detected manifest | `<your build command here>` |
| `repo-description` / `repo-topics` / `repo-homepage` | Draft the text and hand it to the author | Changing repository settings yourself |

Three rules that hold across all of them:

- **Cite the finding.** Every edit you make should be traceable to an id and a
  file:line that repolish reported. If you cannot name one, you are redecorating.
- **Leave the voice alone.** Match the surrounding register, list markers,
  heading depth and line width. A README that suddenly reads like
  documentation-as-a-service is a worse README even when it scores higher.
- **A higher score is not the goal.** The goal is a repository a stranger can
  use. If a change would raise the number without helping that stranger, do not
  make it, and say why.

### Why there is no model inside repolish

If you are wondering whether to suggest wiring an LLM into it: the scoring path
is deliberately model-free, and that is not conservatism. A badge whose number
moves because a model answered differently this morning is worth nothing, and
the same commit has to produce the same score for the number to be comparable
between repositories at all.

The intended arrangement is the one you are already in: **repolish supplies
evidence, you supply judgment.** You have context it structurally cannot have —
the codebase, the user's intent, this conversation. It has determinism you
cannot have. Neither half is improved by moving it into the other.

## Things to get right

- **Show the dry run before `--apply`.** It edits the author's README in place
  and it executes the demo commands. Both of those need to have been seen first.
- **Local and remote scores are not comparable.** Say which one you ran.
- **Do not tune thresholds.** The check set and weights are frozen for v1 so
  scores stay comparable between repositories.
- **`claim-consistency` is the check to take seriously.** It verifies that the
  commands the README promises actually exist — `npm run build` in
  `package.json`, `make test` as a real target, `./scripts/setup.sh` as a real
  file. A README that fails on its first command is where readers leave. Fix
  those first, and fix them by making the claim true or by correcting it — never
  by deleting the line to make the check pass.
- **Report the numbers the tool gave you.** If it says `not scored`, say
  `not scored`.

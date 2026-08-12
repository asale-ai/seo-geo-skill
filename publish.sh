#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Unattended release. Bumps the version, commits everything in the working
# tree with the message you give it, pushes, tags, and pushes the tag — which
# triggers .github/workflows/release.yml.
#
#   ./publish.sh "fix hreflang self-reference detection"
#   ./publish.sh --minor "add UCP auditing"
#   ./publish.sh --version 1.0.0 "first stable release"
#   ./publish.sh --clawhub "publish skills to ClawHub too"
#
# There is no interactive confirmation anywhere. Everything that could need a
# decision is a flag with a documented default.

set -euo pipefail

REPO_SLUG="asale-ai/seo-geo-skill"
BUMP="patch"
EXPLICIT_VERSION=""
DRY_RUN=0
WITH_CLAWHUB=0
SKIP_TESTS=0
MESSAGE=""

BOLD=$(tput bold 2>/dev/null || printf '')
RED=$(tput setaf 1 2>/dev/null || printf '')
GREEN=$(tput setaf 2 2>/dev/null || printf '')
YELLOW=$(tput setaf 3 2>/dev/null || printf '')
RESET=$(tput sgr0 2>/dev/null || printf '')

step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

usage() {
  sed -n '3,20p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'EOF'

Flags:
  --patch | --minor | --major   Which component to bump (default: --patch)
  --version X.Y.Z               Set an exact version instead of bumping
  --clawhub                     Also publish the skills to ClawHub
  --skip-tests                  Skip cargo test (not recommended)
  --dry-run                     Print what would happen; change nothing
  -h, --help                    This text
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --patch) BUMP="patch"; shift ;;
    --minor) BUMP="minor"; shift ;;
    --major) BUMP="major"; shift ;;
    --version) EXPLICIT_VERSION="${2:?--version needs X.Y.Z}"; shift 2 ;;
    --clawhub) WITH_CLAWHUB=1; shift ;;
    --skip-tests) SKIP_TESTS=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) die "unknown flag: $1 (try --help)" ;;
    *) MESSAGE="$1"; shift ;;
  esac
done

[ -n "$MESSAGE" ] || { usage; die "a commit message is required"; }

cd "$(dirname "$0")"

run() {
  if [ "$DRY_RUN" = "1" ]; then
    printf '    %s[dry-run]%s %s\n' "$YELLOW" "$RESET" "$*"
  else
    "$@"
  fi
}

# ------------------------------------------------------------ preflight

step "Preflight"
command -v cargo > /dev/null || die "cargo is not installed"
command -v git   > /dev/null || die "git is not installed"
git rev-parse --git-dir > /dev/null 2>&1 || die "not a git repository"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
info "branch: $BRANCH"

if ! git remote get-url origin > /dev/null 2>&1; then
  die "no 'origin' remote configured"
fi

# Fetching first means a stale local branch fails here rather than after the
# version has already been bumped and committed.
git fetch --quiet origin "$BRANCH" 2>/dev/null || warn "could not fetch origin/$BRANCH"
if git rev-parse --verify --quiet "origin/$BRANCH" > /dev/null; then
  BEHIND=$(git rev-list --count "HEAD..origin/$BRANCH")
  if [ "$BEHIND" != "0" ]; then
    die "local $BRANCH is $BEHIND commit(s) behind origin. Pull, then re-run."
  fi
fi

# ------------------------------------------------------------ version

CURRENT=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
[ -n "$CURRENT" ] || die "could not read the version from Cargo.toml"

if [ -n "$EXPLICIT_VERSION" ]; then
  echo "$EXPLICIT_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
    || die "--version must be X.Y.Z, got: $EXPLICIT_VERSION"
  NEW="$EXPLICIT_VERSION"
else
  IFS=. read -r MAJOR MINOR PATCH <<< "$CURRENT"
  case "$BUMP" in
    major) NEW="$((MAJOR + 1)).0.0" ;;
    minor) NEW="$MAJOR.$((MINOR + 1)).0" ;;
    patch) NEW="$MAJOR.$MINOR.$((PATCH + 1))" ;;
  esac
fi

step "Version $CURRENT -> $NEW"

if git rev-parse --verify --quiet "refs/tags/v$NEW" > /dev/null; then
  die "tag v$NEW already exists. Pass --version with a higher number."
fi
if git ls-remote --exit-code --tags origin "refs/tags/v$NEW" > /dev/null 2>&1; then
  die "tag v$NEW already exists on origin. Pass --version with a higher number."
fi

# ------------------------------------------------------------ tests

if [ "$SKIP_TESTS" = "0" ]; then
  step "Testing"
  if [ "$DRY_RUN" = "1" ]; then
    info "[dry-run] cargo test --locked"
  else
    cargo test --locked || die "tests failed — nothing was committed or tagged"
  fi
else
  warn "skipping tests"
fi

# ------------------------------------------------------------ bump

step "Writing the new version"
if [ "$DRY_RUN" = "0" ]; then
  # Rewrite only the first `version =` line, which is the package's own.
  # awk rather than sed: BSD and GNU sed disagree about the "first match only"
  # idiom, and this has to work on both macOS and Linux runners.
  awk -v new="$NEW" '
    !done && /^version = "/ { sub(/"[^"]*"/, "\"" new "\""); done = 1 }
    { print }
  ' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
  WROTE=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
  [ "$WROTE" = "$NEW" ] || die "Cargo.toml still reads $WROTE after the bump"
  # Keeps Cargo.lock's own record of the package version in step, so
  # `cargo build --locked` in CI does not fail.
  cargo update --workspace --quiet 2>/dev/null || cargo metadata --quiet > /dev/null 2>&1 || true
fi
info "Cargo.toml and Cargo.lock updated"

# ------------------------------------------------------------ commit + push

step "Committing"
if [ "$DRY_RUN" = "0" ]; then
  git add -A
  if git diff --cached --quiet; then
    warn "nothing to commit; tagging the current HEAD"
  else
    git commit -q -m "$MESSAGE" -m "Release v$NEW"
    info "$(git log -1 --oneline)"
  fi
else
  info "[dry-run] git add -A && git commit -m \"$MESSAGE\""
fi

step "Pushing $BRANCH"
run git push origin "$BRANCH"

step "Tagging v$NEW"
run git tag -a "v$NEW" -m "v$NEW: $MESSAGE"
# The tag push is what starts the release workflow, so it goes last: if the
# branch push failed, no build is triggered against code nobody can fetch.
run git push origin "v$NEW"

# ------------------------------------------------------------ watch

step "Release workflow"
if command -v gh > /dev/null 2>&1 && [ "$DRY_RUN" = "0" ]; then
  info "waiting for the run to appear..."
  sleep 8
  RUN_ID=$(gh run list --workflow release.yml --limit 1 --json databaseId \
             --jq '.[0].databaseId' 2>/dev/null || true)
  if [ -n "$RUN_ID" ]; then
    info "watching run $RUN_ID"
    gh run watch "$RUN_ID" --exit-status || die "the release workflow failed.
Inspect it with: gh run view $RUN_ID --log-failed"
    printf '%s\n' "${GREEN}Release v$NEW published.${RESET}"
    gh release view "v$NEW" --json assets --jq '.assets[].name' 2>/dev/null || true
  else
    warn "could not find the workflow run; check https://github.com/$REPO_SLUG/actions"
  fi
else
  info "https://github.com/$REPO_SLUG/actions"
fi

# ------------------------------------------------------------ clawhub

if [ "$WITH_CLAWHUB" = "1" ]; then
  step "Publishing to ClawHub"
  if [ "$DRY_RUN" = "1" ]; then
    info "[dry-run] ./scripts/publish-clawhub.sh $NEW"
  else
    ./scripts/publish-clawhub.sh "$NEW" "$MESSAGE"
  fi
fi

printf '\n%sv%s%s\n' "$GREEN" "$NEW" "$RESET"

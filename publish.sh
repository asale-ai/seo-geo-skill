#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Unattended release. Bumps the version on a release branch, opens a pull
# request against main, lets it auto-merge once the required checks pass, then
# tags the commit that actually landed — which triggers
# .github/workflows/release.yml.
#
#   ./publish.sh "fix hreflang self-reference detection"
#   ./publish.sh --minor "add UCP auditing"
#   ./publish.sh --version 1.0.0 "first stable release"
#   ./publish.sh --clawhub "publish skills to ClawHub too"
#
# main is protected by a ruleset that nobody can bypass, so the direct push
# this script used to do is rejected by the server. Everything still runs
# without a prompt: the ruleset requires a pull request but zero approvals.
#
# There is no interactive confirmation anywhere. Everything that could need a
# decision is a flag with a documented default.

set -euo pipefail

REPO_SLUG="asale-ai/seo-geo-skill"
BASE_BRANCH="main"
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
  sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'EOF'

Flags:
  --patch | --minor | --major   Which component to bump (default: --patch)
  --version X.Y.Z               Set an exact version instead of bumping
  --clawhub                     Also publish the skills to ClawHub
  --skip-tests                  Skip the local cargo test (CI still gates the PR)
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
# gh is no longer optional: opening and merging the pull request is the only
# way onto main.
command -v gh    > /dev/null || die "gh is not installed (brew install gh)"
gh auth status > /dev/null 2>&1 || die "gh is not authenticated; run: gh auth login"
git rev-parse --git-dir > /dev/null 2>&1 || die "not a git repository"

git remote get-url origin > /dev/null 2>&1 || die "no 'origin' remote configured"

START_BRANCH=$(git rev-parse --abbrev-ref HEAD)
info "starting from: $START_BRANCH"

git fetch --quiet origin "$BASE_BRANCH" || die "could not fetch origin/$BASE_BRANCH"

# The ruleset sets strict_required_status_checks_policy, so a pull request
# based on a stale main cannot merge. Failing here beats failing after the
# version has been bumped, committed, and pushed.
BEHIND=$(git rev-list --count "HEAD..origin/$BASE_BRANCH")
if [ "$BEHIND" != "0" ]; then
  die "HEAD is $BEHIND commit(s) behind origin/$BASE_BRANCH. Pull, then re-run."
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

RELEASE_BRANCH="release/v$NEW"

step "Version $CURRENT -> $NEW"

# The tag ruleset forbids deletion, update, and non-fast-forward on refs/tags/v*
# with no bypass actor, so a tag pushed at the wrong commit can never be
# repaired — only abandoned for a higher version. Hence the checks below, and
# hence the tag is created at the very end, against what is actually on main.
if git rev-parse --verify --quiet "refs/tags/v$NEW" > /dev/null; then
  die "tag v$NEW already exists locally. Pass --version with a higher number."
fi
if git ls-remote --exit-code --tags origin "refs/tags/v$NEW" > /dev/null 2>&1; then
  die "tag v$NEW already exists on origin. Pass --version with a higher number."
fi
if git rev-parse --verify --quiet "refs/heads/$RELEASE_BRANCH" > /dev/null; then
  die "branch $RELEASE_BRANCH already exists locally. Delete it, or use --version."
fi

# ------------------------------------------------------------ tests

if [ "$SKIP_TESTS" = "0" ]; then
  step "Testing"
  if [ "$DRY_RUN" = "1" ]; then
    info "[dry-run] cargo test --locked"
  else
    cargo test --locked || die "tests failed — nothing was committed or pushed"
  fi
else
  warn "skipping the local test run; CI still gates the pull request"
fi

# ------------------------------------------------------------ release branch

step "Release branch $RELEASE_BRANCH"
run git switch -c "$RELEASE_BRANCH"

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
    die "nothing to commit — a pull request needs at least one commit"
  fi
  git commit -q -m "$MESSAGE" -m "Release v$NEW"
  info "$(git log -1 --oneline)"
else
  info "[dry-run] git add -A && git commit -m \"$MESSAGE\""
fi

step "Pushing $RELEASE_BRANCH"
run git push -u origin "$RELEASE_BRANCH"

# ------------------------------------------------------------ pull request

step "Opening the pull request"
PR_NUM=""
if [ "$DRY_RUN" = "0" ]; then
  gh pr create \
    --base "$BASE_BRANCH" --head "$RELEASE_BRANCH" \
    --title "$MESSAGE" \
    --body "Release v$NEW.

$MESSAGE

Opened by publish.sh; merges itself once the required checks pass." > /dev/null
  PR_NUM=$(gh pr view "$RELEASE_BRANCH" --json number --jq .number)
  info "#$PR_NUM $(gh pr view "$PR_NUM" --json url --jq .url)"
else
  info "[dry-run] gh pr create --base $BASE_BRANCH --head $RELEASE_BRANCH"
fi

step "Merging"
if [ "$DRY_RUN" = "1" ]; then
  info "[dry-run] gh pr merge --squash --delete-branch"
elif gh pr merge "$PR_NUM" --squash --auto --delete-branch 2>/dev/null; then
  # Auto-merge is the race-free path: GitHub merges the moment the last
  # required check reports green.
  info "auto-merge armed; waiting for the required checks"
else
  # allow_auto_merge is off on the repository, so watch the checks here and
  # merge by hand once they pass.
  warn "auto-merge unavailable; watching the required checks instead"
  gh pr checks "$PR_NUM" --watch --required --fail-fast \
    || die "required checks failed on #$PR_NUM. The release branch is still
open; fix it, push to $RELEASE_BRANCH, and merge the pull request yourself."
  gh pr merge "$PR_NUM" --squash --delete-branch \
    || die "could not merge #$PR_NUM"
fi

# ------------------------------------------------------------ wait for main

step "Waiting for #${PR_NUM:-?} to land"
MERGE_SHA=""
if [ "$DRY_RUN" = "0" ]; then
  STATE=""
  for _ in $(seq 1 180); do
    STATE=$(gh pr view "$PR_NUM" --json state --jq .state 2>/dev/null || echo "")
    case "$STATE" in
      MERGED) break ;;
      CLOSED) die "#$PR_NUM was closed without merging" ;;
    esac
    sleep 10
  done
  [ "$STATE" = "MERGED" ] || die "timed out waiting for #$PR_NUM to merge.
Nothing was tagged. Check: gh pr view $PR_NUM --web"
  MERGE_SHA=$(gh pr view "$PR_NUM" --json mergeCommit --jq .mergeCommit.oid)
  info "merged as ${MERGE_SHA:0:12}"
else
  info "[dry-run] poll until the pull request reports MERGED"
fi

step "Syncing $BASE_BRANCH"
run git switch "$BASE_BRANCH"
run git fetch origin "$BASE_BRANCH"
# The squash rewrote history: the commit built locally is not the commit on
# main. Discarding the local branch is the point, not a side effect.
run git reset --hard "origin/$BASE_BRANCH"
run git branch -D "$RELEASE_BRANCH"

# ------------------------------------------------------------ tag

if [ "$DRY_RUN" = "0" ]; then
  HEAD_SHA=$(git rev-parse HEAD)
  [ "$HEAD_SHA" = "$MERGE_SHA" ] \
    || die "origin/$BASE_BRANCH is at ${HEAD_SHA:0:12}, not the merge commit
${MERGE_SHA:0:12}. Something else landed; nothing was tagged."
  # Last line of defence before an untouchable tag: main really does carry the
  # version this release claims.
  LANDED=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
  [ "$LANDED" = "$NEW" ] \
    || die "main reads version $LANDED, not $NEW. Nothing was tagged."
fi

step "Tagging v$NEW"
if [ "$DRY_RUN" = "0" ]; then
  git tag -a "v$NEW" -m "v$NEW: $MESSAGE" "$MERGE_SHA"
else
  info "[dry-run] git tag -a v$NEW <merge commit>"
fi
run git push origin "v$NEW"

# ------------------------------------------------------------ watch

step "Release workflow"
if [ "$DRY_RUN" = "0" ]; then
  RUN_ID=""
  # Filtering by the tag matters now that CI also runs on every pull request:
  # `--limit 1` on its own can hand back somebody else's run.
  for _ in $(seq 1 24); do
    RUN_ID=$(gh run list --workflow release.yml --branch "v$NEW" --limit 1 \
               --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)
    [ -n "$RUN_ID" ] && break
    sleep 5
  done
  if [ -n "$RUN_ID" ]; then
    info "watching run $RUN_ID"
    gh run watch "$RUN_ID" --exit-status || die "the release workflow failed.
Inspect it with: gh run view $RUN_ID --log-failed
The tag v$NEW is immutable; re-run against a higher version once fixed."
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

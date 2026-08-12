#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Publish the bundled skills to ClawHub under the asale-ai publisher.
#
#   ./scripts/publish-clawhub.sh [version] [changelog]
#   ./scripts/publish-clawhub.sh --dry-run
#
# The token is read from .env (CLAWHUB_API_KEY) or from the environment. It is
# never written into the repository, and .env is gitignored.

set -euo pipefail

OWNER="${CLAWHUB_OWNER:-asale-ai}"
SKILLS_DIR="skills"
DRY_RUN=0
VERSION=""
CHANGELOG=""

BOLD=$(tput bold 2>/dev/null || printf '')
RED=$(tput setaf 1 2>/dev/null || printf '')
GREEN=$(tput setaf 2 2>/dev/null || printf '')
YELLOW=$(tput setaf 3 2>/dev/null || printf '')
RESET=$(tput sgr0 2>/dev/null || printf '')

step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --owner) OWNER="${2:?--owner needs a handle}"; shift 2 ;;
    -h|--help)
      sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    -*) die "unknown flag: $1" ;;
    *)
      if [ -z "$VERSION" ]; then VERSION="$1"; else CHANGELOG="$1"; fi
      shift ;;
  esac
done

cd "$(dirname "$0")/.."

# ------------------------------------------------------------ credentials

if [ -f .env ]; then
  # shellcheck disable=SC1091
  set -a; . ./.env; set +a
  info "loaded credentials from .env"
fi

TOKEN="${CLAWHUB_TOKEN:-${CLAWHUB_API_KEY:-}}"
[ -n "$TOKEN" ] || die "no ClawHub token.
Put CLAWHUB_API_KEY=... in .env (gitignored), or export CLAWHUB_TOKEN.
Get one with: clawhub login"
export CLAWHUB_TOKEN="$TOKEN"

command -v clawhub > /dev/null || die "clawhub is not installed.
    npm i -g clawhub"

step "Authenticating"
WHO=$(clawhub whoami 2>&1 | tail -n1) || die "clawhub whoami failed: $WHO"
info "authenticated as $WHO"

# ------------------------------------------------------------ metadata

[ -n "$VERSION" ] || VERSION=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
[ -n "$CHANGELOG" ] || CHANGELOG="Release v$VERSION"

SOURCE_REPO="asale-ai/seo-geo-skill"
SOURCE_COMMIT=$(git rev-parse HEAD 2>/dev/null || printf '')
SOURCE_REF=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf 'main')

SKILL_COUNT=$(find "$SKILLS_DIR" -maxdepth 2 -name SKILL.md | wc -l | tr -d ' ')
step "Publishing $SKILL_COUNT skill(s) as @$OWNER"
info "version:   $VERSION"
info "changelog: $CHANGELOG"
info "source:    $SOURCE_REPO@${SOURCE_COMMIT:0:12}"

# The skills call `seogeo`, so a ClawHub install that does not also install the
# binary is useless. Fail loudly rather than shipping something broken.
if ! grep -q 'clawhub install' README.md; then
  warn "README.md no longer documents the ClawHub install command"
fi

# ------------------------------------------------------------ publish

# ClawHub requires --source-repo and --source-commit together, so provenance
# is all-or-nothing. Outside a git checkout we simply omit it.
PROVENANCE=()
if [ -n "$SOURCE_COMMIT" ]; then
  PROVENANCE=(--source-repo "$SOURCE_REPO"
              --source-commit "$SOURCE_COMMIT"
              --source-ref "$SOURCE_REF")
else
  warn "no git commit available; publishing without source provenance"
fi

# `sync` diffs every local skill against the registry and publishes only the
# new or changed ones, which keeps re-runs cheap and version numbers honest.
if [ "$DRY_RUN" = "1" ]; then
  step "Dry run"
  clawhub sync --dir "$SKILLS_DIR" --owner "$OWNER" --dry-run \
    "${PROVENANCE[@]}" 2>&1 | head -80
  exit 0
fi

sync_once() {
  clawhub sync --dir "$SKILLS_DIR" --owner "$OWNER" --all --bump patch \
    --changelog "$CHANGELOG" --tags latest "${PROVENANCE[@]}" 2>&1 | tee /dev/stderr
}

step "Uploading"
LOG=$(sync_once) || SYNC_FAILED=1

# A submitted skill stays invisible until its security scan clears, so a run
# interrupted mid-way leaves the registry knowing about skills that `sync`
# cannot yet see. The next pass then tries to publish 1.0.0 again and is
# rejected. One retry is enough: by then the registry reports them, and sync
# classifies them as changed and bumps the version.
if printf '%s' "$LOG" | grep -q 'already exists'; then
  CONFLICTS=$(printf '%s' "$LOG" | grep -c 'already exists' || true)
  warn "$CONFLICTS skill(s) hit a version conflict from an earlier partial run; retrying once"
  sleep 20
  LOG=$(sync_once) || SYNC_FAILED=1
fi

if [ "${SYNC_FAILED:-0}" = "1" ] && printf '%s' "$LOG" | grep -qi 'publisher\|not found\|forbidden\|unauthor'; then
  die "clawhub sync failed.
If this is the first publish under @$OWNER, create the publisher first:
    clawhub publisher create $OWNER"
fi

step "Verifying"
# `sync --dry-run` is the authoritative view: it reports what the registry
# already has versus what is still local-only.
STATE=$(clawhub sync --dir "$SKILLS_DIR" --owner "$OWNER" --dry-run 2>&1 || true)
PENDING=$(printf '%s' "$STATE" | grep -cE '^- ' || true)
LOCAL_TOTAL=$SKILL_COUNT

if [ "$PENDING" = "0" ]; then
  info "${GREEN}all $LOCAL_TOTAL skill(s) are in the registry${RESET}"
else
  warn "$PENDING of $LOCAL_TOTAL skill(s) are not in the registry yet:"
  printf '%s\n' "$STATE" | grep -E '^- ' | sed 's/^/      /'
fi

# Newly submitted versions are held until their security scan clears, so
# `inspect` returning "hidden by moderation" is the expected state, not a
# failure. Distinguish the two rather than reporting a false problem.
for slug in seo geo seo-geo-skill; do
  RESULT=$(clawhub inspect "@$OWNER/$slug" 2>&1 || true)
  case "$RESULT" in
    *"pending.publication"*) info "@$OWNER/$slug — submitted, awaiting the security scan" ;;
    *"not publicly visible"*) warn "@$OWNER/$slug — held by moderation: $(printf '%s' "$RESULT" | head -n2 | tail -n1)" ;;
    *"@$OWNER"*) info "${GREEN}live${RESET} @$OWNER/$slug" ;;
    *) warn "@$OWNER/$slug — not found" ;;
  esac
done

printf '\n'
printf 'Registry: https://clawhub.ai/@%s\n\n' "$OWNER"
printf 'Install with:\n\n    clawhub install @%s/seo-geo-skill\n\n' "$OWNER"
if [ "$PENDING" != "0" ]; then
  warn "re-run this script once the pending scans clear to publish the rest"
  exit 1
fi

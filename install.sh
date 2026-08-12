#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Installer for seogeo — the engine behind the SEO + GEO agent skills.
#
#   curl -fsSL https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.sh | sh
#
# Options (environment variables):
#   SEOGEO_VERSION   tag to install, e.g. v0.1.0 (default: latest release)
#   SEOGEO_BIN_DIR   install directory       (default: ~/.local/bin)
#   SEOGEO_TARGET    agent target for skills (default: all; "none" to skip)
#   SEOGEO_NO_SKILLS set to 1 to install only the binary
#
# POSIX sh on purpose: this runs on Alpine, on minimal CI images, and on
# macOS's ancient bash. No arrays, no [[ ]], no process substitution.

set -eu

REPO="asale-ai/seo-geo-skill"
BIN_NAME="seogeo"
BIN_DIR="${SEOGEO_BIN_DIR:-$HOME/.local/bin}"
TARGET_TOOL="${SEOGEO_TARGET:-all}"

RED=''; GREEN=''; YELLOW=''; BOLD=''; RESET=''
if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
  RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m')
  YELLOW=$(printf '\033[33m'); BOLD=$(printf '\033[1m'); RESET=$(printf '\033[0m')
fi

info() { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
warn() { printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

have() { command -v "$1" > /dev/null 2>&1; }

cleanup() { [ -n "${TMP_DIR:-}" ] && rm -rf "$TMP_DIR"; }
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------- platform

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)

  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux)  os_part="unknown-linux" ;;
    MINGW*|MSYS*|CYGWIN*)
      die "Windows detected. Use install.ps1 instead:
    irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
    *) die "unsupported operating system: $os" ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch_part="x86_64" ;;
    arm64|aarch64) arch_part="aarch64" ;;
    *) die "unsupported architecture: $arch
seogeo ships x86_64 and aarch64 builds. Build from source instead:
    cargo install --git https://github.com/$REPO" ;;
  esac

  if [ "$os_part" = "unknown-linux" ]; then
    # A musl build runs on both glibc and musl systems; a gnu build does not
    # run on Alpine. Detect rather than guess, and fall back to musl, which is
    # the safer of the two.
    if have ldd && ldd --version 2>&1 | grep -qi 'gnu\|glibc'; then
      libc="gnu"
    else
      libc="musl"
    fi
    TARGET="${arch_part}-${os_part}-${libc}"
  else
    TARGET="${arch_part}-${os_part}"
  fi
}

# ------------------------------------------------------------------ helpers

download() {
  url="$1"; dest="$2"
  if have curl; then
    curl -fsSL --retry 3 --retry-delay 2 -o "$dest" "$url" || return 1
  elif have wget; then
    wget -qO "$dest" "$url" || return 1
  else
    die "neither curl nor wget is available"
  fi
}

sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  elif have openssl; then openssl dgst -sha256 "$1" | awk '{print $NF}'
  else return 1
  fi
}

resolve_version() {
  if [ -n "${SEOGEO_VERSION:-}" ]; then
    VERSION="$SEOGEO_VERSION"
    return
  fi
  step "Resolving the latest release"
  api="https://api.github.com/repos/$REPO/releases/latest"
  if have curl; then body=$(curl -fsSL "$api" 2>/dev/null || true)
  else body=$(wget -qO- "$api" 2>/dev/null || true)
  fi
  VERSION=$(printf '%s' "$body" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
  [ -n "$VERSION" ] || die "could not determine the latest release from $api.
Set SEOGEO_VERSION=vX.Y.Z to install a specific version."
}

# ------------------------------------------------------------------- install

main() {
  detect_target
  resolve_version
  NUM_VERSION="${VERSION#v}"

  ASSET="${BIN_NAME}-${NUM_VERSION}-${TARGET}.tar.gz"
  BASE="https://github.com/$REPO/releases/download/$VERSION"

  TMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t seogeo)
  info "  version:  $VERSION"
  info "  platform: $TARGET"
  info "  install:  $BIN_DIR"
  info ""

  step "Downloading $ASSET"
  if ! download "$BASE/$ASSET" "$TMP_DIR/$ASSET"; then
    die "download failed: $BASE/$ASSET
The release may not include a build for $TARGET.
See https://github.com/$REPO/releases/tag/$VERSION"
  fi

  step "Verifying checksum"
  if download "$BASE/SHA256SUMS" "$TMP_DIR/SHA256SUMS"; then
    expected=$(grep " $ASSET\$" "$TMP_DIR/SHA256SUMS" | cut -d' ' -f1 | head -n1)
    actual=$(sha256_of "$TMP_DIR/$ASSET") || {
      warn "no SHA-256 tool found (sha256sum, shasum, or openssl); skipping verification"
      expected=""; actual=""
    }
    if [ -n "$expected" ] && [ -n "$actual" ]; then
      if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $ASSET
  expected: $expected
  actual:   $actual
Nothing was installed. This could mean a corrupted download or a tampered
artifact — re-run, and report it if it persists."
      fi
      info "  ok: $actual"
    elif [ -z "$expected" ]; then
      warn "$ASSET is not listed in SHA256SUMS; continuing unverified"
    fi
  else
    warn "could not download SHA256SUMS; continuing unverified"
  fi

  step "Extracting"
  tar xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR" || die "could not extract $ASSET"
  SRC="$TMP_DIR/${BIN_NAME}-${NUM_VERSION}-${TARGET}/$BIN_NAME"
  [ -f "$SRC" ] || SRC=$(find "$TMP_DIR" -name "$BIN_NAME" -type f | head -n1)
  [ -f "$SRC" ] || die "the archive did not contain a $BIN_NAME binary"

  step "Installing to $BIN_DIR"
  mkdir -p "$BIN_DIR" || die "cannot create $BIN_DIR
Set SEOGEO_BIN_DIR to a writable directory and re-run."
  # Install to a temp name and rename, so a running seogeo is never truncated.
  install_tmp="$BIN_DIR/.$BIN_NAME.$$"
  cp "$SRC" "$install_tmp" || die "cannot write to $BIN_DIR
Set SEOGEO_BIN_DIR to a writable directory and re-run."
  chmod 755 "$install_tmp"
  mv -f "$install_tmp" "$BIN_DIR/$BIN_NAME" || die "cannot replace $BIN_DIR/$BIN_NAME"

  if ! "$BIN_DIR/$BIN_NAME" --version > /dev/null 2>&1; then
    die "the installed binary did not run.
This usually means an architecture mismatch — detected $TARGET."
  fi
  installed=$("$BIN_DIR/$BIN_NAME" --version)
  info "  ${GREEN}installed${RESET} $installed"

  if [ "${SEOGEO_NO_SKILLS:-}" != "1" ] && [ "$TARGET_TOOL" != "none" ]; then
    info ""
    step "Installing skills"
    "$BIN_DIR/$BIN_NAME" install --target "$TARGET_TOOL" || \
      warn "skill installation failed; run '$BIN_NAME install --target all' by hand"
  fi

  info ""
  case ":$PATH:" in
    *":$BIN_DIR:"*)
      info "${GREEN}Done.${RESET} Try: ${BOLD}$BIN_NAME install --list${RESET}"
      ;;
    *)
      shell_name=$(basename "${SHELL:-sh}")
      case "$shell_name" in
        zsh)  rc="~/.zshrc" ;;
        bash) rc="~/.bashrc" ;;
        fish) rc="~/.config/fish/config.fish" ;;
        *)    rc="your shell profile" ;;
      esac
      info "${GREEN}Done.${RESET}"
      info ""
      info "${YELLOW}$BIN_DIR is not on your PATH.${RESET} The skills call '$BIN_NAME' by name,"
      info "so add it before using them:"
      info ""
      if [ "$shell_name" = "fish" ]; then
        info "  ${BOLD}fish_add_path $BIN_DIR${RESET}"
      else
        info "  ${BOLD}echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> $rc${RESET}"
        info "  ${BOLD}exec \$SHELL${RESET}"
      fi
      ;;
  esac
}

main "$@"

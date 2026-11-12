#!/usr/bin/env bash
# Install Violet from a GitHub release, or build a local checkout when run there.
set -euo pipefail

REPOSITORY="tiagohierath/violet"
INSTALL_VERSION="${VIOLET_VERSION:-latest}"
BIN_DIR="${VIOLET_BIN_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
TARGET="$BIN_DIR/violet"

say() { printf '%s\n' "$*"; }
fail() { printf 'violet installer: %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
  --help|-h) say "Usage: ./install.sh [--uninstall]"; exit 0 ;;
  --uninstall)
    rm -f "$TARGET" "$DATA_HOME/applications/violet.desktop"
    say "Removed Violet. Your drawings remain untouched."
    exit 0
    ;;
  "") ;;
  *) fail "unknown option: $1" ;;
esac

case "$(uname -s)" in Linux) ;; *) fail "this installer currently supports Linux" ;; esac
case "$(uname -m)" in x86_64|amd64) ARCH="x86_64" ;; *) fail "unsupported CPU architecture: $(uname -m)" ;; esac

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/violet-install.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT HUP INT TERM

build_local() {
  [ -f "$SCRIPT_DIR/flake.nix" ] && command -v nix >/dev/null 2>&1 || return 1
  say "Building Violet from the local checkout…"
  (cd "$SCRIPT_DIR" && nix build --impure --out-link "$TEMP_DIR/result")
  cp "$TEMP_DIR/result/bin/violet" "$TEMP_DIR/violet"
  cp "$TEMP_DIR/result/share/applications/violet.desktop" "$TEMP_DIR/violet.desktop"
}

download_release() {
  if [ "$INSTALL_VERSION" = latest ]; then
    URL="https://github.com/$REPOSITORY/releases/latest/download/violet-linux-$ARCH.tar.gz"
  else
    URL="https://github.com/$REPOSITORY/releases/download/$INSTALL_VERSION/violet-linux-$ARCH.tar.gz"
  fi
  say "Downloading Violet for Linux/$ARCH…"
  command -v curl >/dev/null 2>&1 || fail "curl is required"
  curl --fail --location --proto '=https' --tlsv1.2 "$URL" -o "$TEMP_DIR/violet.tar.gz"
  curl --fail --location --proto '=https' --tlsv1.2 "$URL.sha256" -o "$TEMP_DIR/violet.tar.gz.sha256"
  command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"
  (cd "$TEMP_DIR" && sha256sum --check violet.tar.gz.sha256)
  tar -xzf "$TEMP_DIR/violet.tar.gz" -C "$TEMP_DIR"
  test -x "$TEMP_DIR/violet" || fail "the release did not contain violet"
}

if ! build_local; then download_release; fi

mkdir -p "$BIN_DIR" "$DATA_HOME/applications"
install -m 755 "$TEMP_DIR/violet" "$TARGET"
install -m 644 "$TEMP_DIR/violet.desktop" "$DATA_HOME/applications/violet.desktop"
say "Installed Violet: $TARGET"
say "Launch it from your applications menu or run: violet"

#!/usr/bin/env bash
# build Lilyfield as a Nix package and install it for this user; rerun after code changes to update
set -eu
cd "$(dirname "$0")/.."

# a flake only sees files git knows about; -N makes them visible without staging their content
git add -N src Cargo.toml Cargo.lock
[ -d assets ] && git add -N assets

SHARE="$HOME/.local/share/lilyfield"
mkdir -p "$SHARE" "$HOME/.local/bin" "$HOME/.local/share/applications"
# the out-link is a GC root, so nix-collect-garbage keeps the installed build
nix build . -o "$SHARE/result"
ln -sfn "$SHARE/result/bin/lilyfield" "$HOME/.local/bin/lilyfield"
ln -sfn "$SHARE/result/share/applications/lilyfield.desktop" "$HOME/.local/share/applications/lilyfield.desktop"
readlink -e "$HOME/.local/bin/lilyfield"

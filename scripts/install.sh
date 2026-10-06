#!/usr/bin/env bash
# Build and install Violet.
set -euo pipefail
cd "$(dirname "$0")/.."

violet_data="${XDG_DATA_HOME:-$HOME/.local/share}"
violet_share="$violet_data/violet"
mkdir -p "$violet_share" "$HOME/.local/bin" "$violet_data/applications"
# Include untracked source without staging it or copying build artifacts.
VIOLET_SOURCE="$PWD" nix build --impure --expr '
  let
    source = builtins.path {
      path = builtins.getEnv "VIOLET_SOURCE";
      name = "violet-source";
      filter = path: type: !(builtins.elem (baseNameOf path) [ ".git" "target" "result" ]);
    };
    flake = builtins.getFlake (toString source);
  in flake.packages.${builtins.currentSystem}.default
' --out-link "$violet_share/result"

test -x "$violet_share/result/bin/violet"
test -r "$violet_share/result/share/applications/violet.desktop"
ln -sfn "$violet_share/result/bin/violet" "$HOME/.local/bin/violet"
ln -sfn "$violet_share/result/share/applications/violet.desktop" "$violet_data/applications/violet.desktop"
readlink -e "$HOME/.local/bin/violet"

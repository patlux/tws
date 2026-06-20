#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
bin_dir="${TWS_LOCAL_BIN_DIR:-$HOME/.local/bin}"
bin_name="${TWS_LOCAL_BIN_NAME:-tws}"

cd "$repo_root"

if command -v cargo >/dev/null 2>&1; then
  cargo build
elif command -v nix >/dev/null 2>&1; then
  nix --extra-experimental-features nix-command --extra-experimental-features flakes \
    shell nixpkgs#cargo nixpkgs#rustc -c cargo build
else
  printf 'error: neither cargo nor nix found in PATH\n' >&2
  exit 127
fi

install -d "$bin_dir"
install -m 0755 target/debug/tws "$bin_dir/$bin_name"

printf 'installed %s\n' "$bin_dir/$bin_name"
"$bin_dir/$bin_name" --help >/dev/null

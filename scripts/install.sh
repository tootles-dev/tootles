#!/usr/bin/env sh
# Build and install from this checkout. No downloads are executed as scripts.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
install_root=${TOOTLES_INSTALL_ROOT:-"$HOME/.local"}
if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'Rust 1.90+ is required. Install it from https://rustup.rs, then run this script again.' >&2
    exit 1
fi
cargo install --locked --path "$project_dir" --root "$install_root"
printf '\nInstalled %s/bin/tootles\n' "$install_root"
printf 'Add %s/bin to PATH if needed, then run: tootles --demo\n' "$install_root"

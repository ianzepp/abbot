#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "--clean" ]]; then
    echo "Cleaning build artifacts..."
    cargo clean
fi

for crate in daemon cli monitor tui; do
    echo "Installing $crate..."
    cargo install --path "$crate"
done

echo ""
echo "Installed binaries:"

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
for bin in abbotd abbot abbot-monitor abbot-tui; do
    path="$cargo_bin/$bin"
    if [[ -f "$path" ]]; then
        size=$(du -h "$path" | cut -f1)
        printf "  %-20s %s\n" "$bin" "$size"
    fi
done

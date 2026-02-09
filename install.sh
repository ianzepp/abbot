#!/usr/bin/env bash
set -euo pipefail

# Crate-to-binary mapping
declare -A CRATE_BIN=(
    [daemon]=abbotd
    [cli]=abbot
    [monitor]=abbot-monitor
    [tui]=abbot-tui
)

ALL_CRATES=(daemon cli monitor tui)

if [[ "${1:-}" == "--clean" ]]; then
    echo "Cleaning build artifacts..."
    cargo clean
    shift
fi

# If crate names given, install only those; otherwise install all
if [[ $# -gt 0 ]]; then
    crates=("$@")
else
    crates=("${ALL_CRATES[@]}")
fi

for crate in "${crates[@]}"; do
    if [[ -z "${CRATE_BIN[$crate]+x}" ]]; then
        echo "Unknown crate: $crate (valid: ${ALL_CRATES[*]})" >&2
        exit 1
    fi
    echo "Installing $crate..."
    cargo install --path "$crate"
done

echo ""
echo "Installed binaries:"

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
for crate in "${crates[@]}"; do
    bin="${CRATE_BIN[$crate]}"
    path="$cargo_bin/$bin"
    if [[ -f "$path" ]]; then
        size=$(du -h "$path" | cut -f1)
        printf "  %-20s %s\n" "$bin" "$size"
    fi
done

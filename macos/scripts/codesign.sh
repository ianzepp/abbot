#!/bin/bash
set -euo pipefail

# Code sign the Abbot.app bundle and its embedded binaries.
# Usage: ./macos/scripts/codesign.sh <path-to-Abbot.app> <signing-identity>
#
# The signing identity is typically "Developer ID Application: Name (TEAM_ID)"
# or just the Team ID if using --sign with keychain lookup.

APP_PATH="${1:?Usage: codesign.sh <path-to-Abbot.app> <signing-identity>}"
IDENTITY="${2:?Usage: codesign.sh <path-to-Abbot.app> <signing-identity>}"

if [ ! -d "$APP_PATH" ]; then
    echo "ERROR: $APP_PATH not found"
    exit 1
fi

RESOURCES="$APP_PATH/Contents/Resources"
ENTITLEMENTS="$(cd "$(dirname "$0")/.." && pwd)/Abbot/Abbot.entitlements"

echo "==> Signing embedded binaries..."
for binary in abbot abbotd; do
    if [ -f "$RESOURCES/$binary" ]; then
        codesign --force --options runtime \
            --sign "$IDENTITY" \
            --timestamp \
            "$RESOURCES/$binary"
        echo "    Signed: $binary"
    fi
done

echo "==> Signing app bundle..."
codesign --force --options runtime \
    --sign "$IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    --timestamp \
    --deep \
    "$APP_PATH"

echo "==> Verifying..."
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "==> Code signing complete."

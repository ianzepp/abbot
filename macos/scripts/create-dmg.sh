#!/bin/bash
set -euo pipefail

# Package Abbot.app into a .dmg for distribution.
# Usage: ./macos/scripts/create-dmg.sh <path-to-Abbot.app> [output.dmg]

APP_PATH="${1:?Usage: create-dmg.sh <path-to-Abbot.app> [output.dmg]}"
OUTPUT="${2:-Abbot.dmg}"

if [ ! -d "$APP_PATH" ]; then
    echo "ERROR: $APP_PATH not found"
    exit 1
fi

STAGING=$(mktemp -d)
trap "rm -rf '$STAGING'" EXIT

echo "==> Creating DMG staging area..."
cp -R "$APP_PATH" "$STAGING/Abbot.app"
ln -s /Applications "$STAGING/Applications"

echo "==> Creating DMG..."
hdiutil create \
    -volname "Abbot" \
    -srcfolder "$STAGING" \
    -ov \
    -format UDZO \
    "$OUTPUT"

echo "==> DMG created: $OUTPUT"

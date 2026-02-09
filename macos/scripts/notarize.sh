#!/bin/bash
set -euo pipefail

# Submit a .dmg to Apple's notary service and staple the result.
# Usage: ./macos/scripts/notarize.sh <path-to.dmg>
#
# Required environment variables:
#   APPLE_ID                   - Apple ID email
#   APPLE_TEAM_ID              - Apple Developer Team ID
#   APPLE_APP_SPECIFIC_PASSWORD - App-specific password for notarization

DMG_PATH="${1:?Usage: notarize.sh <path-to.dmg>}"

: "${APPLE_ID:?Set APPLE_ID environment variable}"
: "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID environment variable}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?Set APPLE_APP_SPECIFIC_PASSWORD environment variable}"

if [ ! -f "$DMG_PATH" ]; then
    echo "ERROR: $DMG_PATH not found"
    exit 1
fi

echo "==> Submitting for notarization..."
xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait

echo "==> Stapling notarization ticket..."
xcrun stapler staple "$DMG_PATH"

echo "==> Verifying staple..."
xcrun stapler validate "$DMG_PATH"

echo "==> Notarization complete."

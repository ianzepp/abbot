#!/bin/bash
set -euo pipefail

# Build the Abbot macOS app for local development.
# Usage: ./macos/scripts/build-app.sh [--release]

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MACOS_DIR="$REPO_ROOT/macos"
BUILD_DIR="$MACOS_DIR/build"
CONFIG="Debug"

if [[ "${1:-}" == "--release" ]]; then
    CONFIG="Release"
fi

echo "==> Building Rust binaries..."
cd "$REPO_ROOT"
cargo build -p abbot-daemon --release

echo "==> Building web UI..."
if [ -d "$REPO_ROOT/web" ]; then
    cd "$REPO_ROOT/web"
    npm ci
    npm run build
fi

echo "==> Building Swift app ($CONFIG)..."
cd "$MACOS_DIR"
xcodebuild -project Abbot.xcodeproj \
    -scheme Abbot \
    -configuration "$CONFIG" \
    -derivedDataPath "$BUILD_DIR" \
    build

# Find the built .app
APP_PATH=$(find "$BUILD_DIR" -name "Abbot.app" -type d | head -1)
if [ -z "$APP_PATH" ]; then
    echo "ERROR: Abbot.app not found in build output"
    exit 1
fi

echo "==> Assembling bundle resources..."
RESOURCES="$APP_PATH/Contents/Resources"
mkdir -p "$RESOURCES"

# Copy Rust binaries
cp "$REPO_ROOT/target/release/abbot" "$RESOURCES/abbot"
cp "$REPO_ROOT/target/release/abbotd" "$RESOURCES/abbotd"
chmod +x "$RESOURCES/abbot" "$RESOURCES/abbotd"

# Copy web dist
if [ -d "$REPO_ROOT/web/dist" ]; then
    rm -rf "$RESOURCES/web-dist"
    cp -R "$REPO_ROOT/web/dist" "$RESOURCES/web-dist"
fi

echo "==> Build complete: $APP_PATH"
echo "    Run with: open '$APP_PATH'"

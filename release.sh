#!/usr/bin/env bash
set -euo pipefail

USAGE="Usage: ./release.sh <major|minor|patch> [--force]"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
BUMP=""
FORCE=false

for arg in "$@"; do
    case "$arg" in
        major|minor|patch) BUMP="$arg" ;;
        --force) FORCE=true ;;
        -h|--help) echo "$USAGE"; exit 0 ;;
        *) echo "Unknown argument: $arg"; echo "$USAGE"; exit 1 ;;
    esac
done

if [[ -z "$BUMP" ]]; then
    echo "$USAGE"
    exit 1
fi

# ---------------------------------------------------------------------------
# Read current version from workspace Cargo.toml
# ---------------------------------------------------------------------------
CARGO_TOML="Cargo.toml"
CURRENT=$(grep -m1 'version *= *"' "$CARGO_TOML" | sed 's/.*"\(.*\)".*/\1/')

if [[ -z "$CURRENT" ]]; then
    echo "Error: could not read version from $CARGO_TOML"
    exit 1
fi

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP" in
    major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
    minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
    patch) PATCH=$((PATCH + 1)) ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
TAG="v${NEW_VERSION}"

echo "Current version: $CURRENT"
echo "New version:     $NEW_VERSION ($BUMP bump)"
echo "Tag:             $TAG"
echo ""

# ---------------------------------------------------------------------------
# Check for uncommitted changes
# ---------------------------------------------------------------------------
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: working tree has uncommitted changes. Commit or stash first."
    exit 1
fi

# ---------------------------------------------------------------------------
# Check if tag already exists
# ---------------------------------------------------------------------------
TAG_EXISTS=false
if git rev-parse "$TAG" >/dev/null 2>&1; then
    TAG_EXISTS=true
fi

if [[ "$TAG_EXISTS" == true && "$FORCE" == false ]]; then
    echo "Error: tag $TAG already exists. Use --force to delete and recreate."
    exit 1
fi

if [[ "$TAG_EXISTS" == true && "$FORCE" == true ]]; then
    echo "Force mode: cleaning up existing tag and release..."

    # Delete remote tag
    git push origin ":refs/tags/$TAG" 2>/dev/null || true

    # Delete local tag
    git tag -d "$TAG" 2>/dev/null || true

    # Delete release on abbot-releases (ignore errors if it doesn't exist)
    gh release delete "$TAG" --repo ianzepp/abbot-releases --yes 2>/dev/null || true

    echo "Cleaned up $TAG"
    echo ""
fi

# ---------------------------------------------------------------------------
# Bump version in Cargo.toml
# ---------------------------------------------------------------------------
sed -i.bak "s/^version = \"$CURRENT\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"
rm -f "$CARGO_TOML.bak"

# Regenerate Cargo.lock
cargo check --quiet 2>/dev/null

echo "Updated $CARGO_TOML: $CURRENT -> $NEW_VERSION"

# ---------------------------------------------------------------------------
# Commit, tag, push
# ---------------------------------------------------------------------------
git add "$CARGO_TOML" Cargo.lock
git commit -m "Release $TAG"
git tag "$TAG"

echo ""
echo "Pushing to origin..."
git push origin main
git push origin "$TAG"

echo ""
echo "Done! Release workflow triggered for $TAG."
echo "Watch progress: https://github.com/ianzepp/abbot/actions"

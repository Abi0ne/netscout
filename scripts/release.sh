#!/usr/bin/env bash
# Publish a NetScout release on GitHub: the app installs it through its
# updater (apple/Sources/NetScout/Updater.swift), which looks for the latest
# release tagged v<version> carrying NetScout.zip.
#
#   1. bump `version` in Cargo.toml ([workspace.package]) and commit
#   2. scripts/release.sh [notes.md]     # notes default to GitHub's generated ones
#
# Requires: a clean tree on main, gh logged in with push access.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
TAG="v$VERSION"
NOTES="${1:-}"
cd "$ROOT"

[ -n "$VERSION" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
[ "$(git branch --show-current)" = main ] || { echo "release from main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the working tree is not clean" >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "tag $TAG already exists: bump the version in Cargo.toml" >&2; exit 1
fi

"$HERE/build-app.sh"
BUILT="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' apple/build/NetScout.app/Contents/Info.plist)"
[ "$BUILT" = "$VERSION" ] || { echo "built $BUILT, expected $VERSION" >&2; exit 1; }

mkdir -p dist
ZIP="dist/NetScout.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent apple/build/NetScout.app "$ZIP"

echo ">> tagging $TAG and pushing"
git tag -a "$TAG" -m "NetScout $VERSION"
git push origin main "$TAG"

echo ">> publishing the release"
if [ -n "$NOTES" ]; then
  gh release create "$TAG" "$ZIP" --title "NetScout $VERSION" --notes-file "$NOTES"
else
  gh release create "$TAG" "$ZIP" --title "NetScout $VERSION" --generate-notes
fi
echo ">> released NetScout $VERSION"

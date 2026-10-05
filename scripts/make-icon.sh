#!/usr/bin/env bash
# Regenerate apple/Resources/AppIcon.icns from scripts/make-icon.swift.
# Run after changing the drawing; build-app.sh copies the .icns into the app.
#
#   scripts/make-icon.sh
#
# Requires: Swift, sips and iconutil (all part of macOS / Command Line Tools).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
OUT="$ROOT/apple/Resources/AppIcon.icns"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

swift "$HERE/make-icon.swift" "$WORK/master.png"
SET="$WORK/AppIcon.iconset"
mkdir -p "$SET"
for px in 16 32 128 256 512; do
  sips -z "$px" "$px" "$WORK/master.png" --out "$SET/icon_${px}x${px}.png" >/dev/null
  sips -z $((px * 2)) $((px * 2)) "$WORK/master.png" --out "$SET/icon_${px}x${px}@2x.png" >/dev/null
done
mkdir -p "$(dirname "$OUT")"
iconutil -c icns "$SET" -o "$OUT"
echo ">> wrote $OUT"

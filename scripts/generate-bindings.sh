#!/usr/bin/env bash
# Regenerate the UniFFI bindings (checked into the repo) from the current core
# API. Run this any time the core's public types change. Swift output is laid
# out for the SwiftPM package in apple/: the Swift file in
# Sources/NetScoutCore, the C header + modulemap in Sources/netscout_coreFFI.
#
#   scripts/generate-bindings.sh [swift|kotlin]
#
# Requires: rustup with the host toolchain. On a macOS host this emits the
# bindings the Xcode project imports; the same script with "kotlin" (later)
# emits the Android bindings.
set -euo pipefail

LANG="${1:-swift}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CORE="$ROOT/core"
NAME="netscout_core"

case "$LANG" in
  swift)  OUT="$(mktemp -d)"; trap 'rm -rf "$OUT"' EXIT ;;
  kotlin) OUT="$ROOT/android/generated" ;;
  *) echo "usage: $0 [swift|kotlin]" >&2; exit 2 ;;
esac

cd "$CORE"
echo ">> cargo build --release (host)"
cargo build --release

# Workspace-aware target dir (cargo metadata prints it in JSON).
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory" *": *"//p' | sed 's/".*//')"
[ -n "$TARGET_DIR" ] || TARGET_DIR="$ROOT/target"
BINDGEN="$TARGET_DIR/release/netscout-core-bindgen"
LIB="$TARGET_DIR/release/lib${NAME}.dylib"
[ -f "$LIB" ] || LIB="$TARGET_DIR/release/lib${NAME}.so"
[ -f "$LIB" ] || { echo "could not find built library (looked in $TARGET_DIR/release)" >&2; exit 1; }

mkdir -p "$OUT"
echo ">> generating $LANG bindings from $LIB"
"$BINDGEN" generate --library "$LIB" --language "$LANG" --out-dir "$OUT"

if [ "$LANG" = swift ]; then
  APPLE="$ROOT/apple/Sources"
  mv "$OUT/${NAME}.swift" "$APPLE/NetScoutCore/${NAME}.swift"
  mv "$OUT/${NAME}FFI.h" "$APPLE/netscout_coreFFI/include/${NAME}FFI.h"
  mv "$OUT/${NAME}FFI.modulemap" "$APPLE/netscout_coreFFI/include/module.modulemap"
  echo ">> swift bindings written to $APPLE"
else
  echo ">> $LANG bindings written to $OUT"
fi

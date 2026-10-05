#!/usr/bin/env bash
# Build the netscout-core universal (x86_64 + arm64) dynamic library, generate
# the Swift bindings, and package everything into an XCFramework under
# apple/libs/. Run on a macOS host with Xcode + the two Rust targets installed:
#
#   rustup target add x86_64-apple-darwin aarch64-apple-darwin
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CORE="$ROOT/core"
OUT="$ROOT/apple/libs"
NAME="netscout_core"
BINDGEN_NAME="netscout-core-bindgen"

cd "$CORE"
mkdir -p "$OUT"
rm -rf "$OUT/NetScoutCore.xcframework"

echo ">> cargo build --release (x86_64 + aarch64, macOS)"
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

# Host build provides the bindgen tool + a dylib to read metadata from.
echo ">> cargo build --release (host, for bindgen)"
cargo build --release
BINDGEN="target/release/${BINDGEN_NAME}"
HOST_LIB="target/release/lib${NAME}.dylib"
[ -f "$HOST_LIB" ] || { echo "missing $HOST_LIB" >&2; exit 1; }

echo ">> generating Swift bindings"
mkdir -p "$OUT/Generated"
"$BINDGEN" generate --library "$HOST_LIB" --language swift --out-dir "$OUT/Generated"

echo ">> assembling XCFramework"
xcodebuild -create-xcframework \
  -library "target/x86_64-apple-darwin/release/lib${NAME}.dylib"   -map x86_64 \
  -library "target/aarch64-apple-darwin/release/lib${NAME}.dylib"  -map arm64 \
  -output "$OUT/NetScoutCore.xcframework"

echo ">> done: $OUT/NetScoutCore.xcframework (+ Swift bindings in $OUT/Generated)"

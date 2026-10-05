#!/usr/bin/env bash
# Build NetScout.app for Apple Silicon with the Command Line Tools only (no
# Xcode project): Rust static library → SwiftPM build → .app bundle → ad-hoc
# signature.
#
#   scripts/build-app.sh            # release build
#   open apple/build/NetScout.app
#
# Requires: rustup with aarch64-apple-darwin, Swift (Command Line Tools).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
APPLE="$ROOT/apple"
APP="$APPLE/build/NetScout.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"

echo ">> cargo build --release (aarch64-apple-darwin)"
(cd "$ROOT" && cargo build --release --target aarch64-apple-darwin -p netscout-core --lib)

echo ">> swift build -c release"
swift build -c release --package-path "$APPLE" --arch arm64
BIN="$(swift build -c release --package-path "$APPLE" --arch arm64 --show-bin-path)/NetScout"

echo ">> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/NetScout"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>NetScout</string>
    <key>CFBundleDisplayName</key><string>NetScout</string>
    <key>CFBundleIdentifier</key><string>dev.netscout.NetScout</string>
    <key>CFBundleExecutable</key><string>NetScout</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>NetScout analizza la rete locale per trovare i dispositivi collegati.</string>
</dict>
</plist>
PLIST

echo ">> ad-hoc signing"
codesign --force --sign - --timestamp=none "$APP"

echo ">> done: $APP"

#!/usr/bin/env bash
# Build the Linux app (linux/, GTK4 + libadwaita) and package it as
# dist/netscout_<version>_<arch>.deb. Run on Linux (e.g. Ubuntu 25.04+).
#
#   scripts/build-deb.sh
#
# Requires: cargo, libgtk-4-dev (>= 4.16), libadwaita-1-dev (>= 1.7),
# dpkg-deb. Install the result with: sudo apt install ./dist/netscout_*.deb
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
ARCH="$(dpkg --print-architecture)"
APP_ID="io.github.abi0ne.NetScout"
[ -n "$VERSION" ] || { echo "no version in Cargo.toml" >&2; exit 1; }

echo ">> building netscout $VERSION ($ARCH)"
cargo build --release --manifest-path "$ROOT/linux/Cargo.toml"
TARGET="${CARGO_TARGET_DIR:-$ROOT/linux/target}"

PKG="$(mktemp -d)"
trap 'rm -rf "$PKG"' EXIT
install -Dm755 "$TARGET/release/netscout" "$PKG/usr/bin/netscout"
install -Dm644 "$ROOT/linux/data/$APP_ID.desktop" "$PKG/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$ROOT/linux/data/$APP_ID.metainfo.xml" "$PKG/usr/share/metainfo/$APP_ID.metainfo.xml"
for icon in "$ROOT"/linux/data/icons/hicolor/*/apps/$APP_ID.png; do
  size="$(basename "$(dirname "$(dirname "$icon")")")"
  install -Dm644 "$icon" "$PKG/usr/share/icons/hicolor/$size/apps/$APP_ID.png"
done

mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<EOF
Package: netscout
Version: $VERSION
Architecture: $ARCH
Maintainer: Giulio Mancarella <giulio.mancarella@gmail.com>
Section: net
Priority: optional
Depends: libc6, libgtk-4-1 (>= 4.16), libadwaita-1-0 (>= 1.7)
Suggests: expect, openssh-client, telnet, remmina
Homepage: https://github.com/Abi0ne/netscout
Installed-Size: $(du -sk "$PKG/usr" | cut -f1)
Description: Fast, non-privileged LAN network scanner
 NetScout finds the devices on the local network and tells what they are:
 IP address, MAC, vendor, name, device type, latency and open ports. It
 saves scans as profiles, compares them, and wakes devices with
 Wake-on-LAN. No administrator privileges needed.
EOF

mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/netscout_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$OUT" >/dev/null
echo ">> wrote $OUT"

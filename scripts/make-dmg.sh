#!/usr/bin/env bash
# Package apple/build/NetScout.app as a drag-to-install disk image:
# NetScout.app, a link to /Applications and a note on the first launch.
#
#   scripts/make-dmg.sh             # → dist/NetScout-<version>.dmg
#
# The app carries an ad-hoc signature (no Developer ID), so Gatekeeper asks
# the user to confirm it the first time; the note in the image explains how.
# Run scripts/build-app.sh first (scripts/release.sh does both).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
APP="$ROOT/apple/build/NetScout.app"
[ -d "$APP" ] || { echo "no $APP: run scripts/build-app.sh first" >&2; exit 1; }
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="$ROOT/dist/NetScout-$VERSION.dmg"

codesign --verify --deep --strict "$APP"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/NetScout.app"
ln -s /Applications "$STAGE/Applicazioni"
cat > "$STAGE/Leggimi.txt" <<'TEXT'
NetScout — installazione

1. Trascina NetScout sulla cartella Applicazioni.
2. Apri NetScout da Applicazioni.

NetScout non è firmato con un certificato Apple Developer ID, quindi al
primo avvio macOS lo blocca dicendo che non può verificarne lo sviluppatore.
Per aprirlo comunque (serve una volta sola):

  • macOS 15 e successivi: chiudi l'avviso, apri Impostazioni di Sistema →
    Privacy e sicurezza, scorri fino a «NetScout è stato bloccato» e premi
    «Apri comunque», poi conferma.
  • macOS 14: clic destro (o Ctrl-clic) su NetScout → Apri → Apri.

Alla prima scansione macOS chiede il permesso di accedere alla rete locale:
concedilo, altrimenti NetScout non trova i dispositivi.

Gli aggiornamenti successivi arrivano dall'app stessa
(NetScout → Impostazioni… → Aggiornamento).
TEXT

mkdir -p "$ROOT/dist"
rm -f "$DMG"
hdiutil create -quiet -volname "NetScout $VERSION" -srcfolder "$STAGE" \
  -fs APFS -format UDZO -ov "$DMG"
hdiutil verify -quiet "$DMG"
echo ">> done: $DMG"

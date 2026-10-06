#!/usr/bin/env bash
# Build the Linux .deb of a commit on a Linux host over SSH and copy it back
# to dist/. Used by scripts/release.sh, which runs on the Mac.
#
#   scripts/remote-deb.sh [commit]        # default: HEAD
#
# The host is $NETSCOUT_LINUX_HOST (an ssh destination, default "reika"); it
# needs cargo (in ~/.cargo/bin or on PATH), libgtk-4-dev, libadwaita-1-dev
# and dpkg-deb. The source is the commit itself (`git archive`), never the
# working tree; the cargo target directory is kept between builds.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
HOST="${NETSCOUT_LINUX_HOST:-reika}"
COMMIT="${1:-HEAD}"
REMOTE='~/.cache/netscout-release'
cd "$ROOT"

echo ">> building the .deb of $(git rev-parse --short "$COMMIT") on $HOST"
git archive --format=tar "$COMMIT" | ssh "$HOST" "
  set -e
  rm -rf $REMOTE/src && mkdir -p $REMOTE/src && tar -x -C $REMOTE/src
  export PATH=\$HOME/.cargo/bin:\$PATH CARGO_TARGET_DIR=$REMOTE/target
  cd $REMOTE/src && rm -rf dist && scripts/build-deb.sh
"
mkdir -p dist
DEB="$(ssh "$HOST" "ls $REMOTE/src/dist/netscout_*.deb")"
scp -q "$HOST:$DEB" dist/
echo ">> copied dist/$(basename "$DEB")"

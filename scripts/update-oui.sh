#!/usr/bin/env bash
# Regenerate core/data/oui.tsv — the MAC-prefix → vendor table embedded in the
# core — from the IEEE MA-L registry (the public OUI listing).
#
#   scripts/update-oui.sh
#
# Output: one "AABBCC<TAB>Organization Name" line per 24-bit OUI, sorted by
# prefix. Requires curl and python3 (both ship with macOS).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/../core/data/oui.tsv"
URL="https://standards-oui.ieee.org/oui/oui.csv"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo ">> downloading $URL"
# Note: the IEEE server rejects some browser user agents (HTTP 418); curl's
# default one is accepted.
curl -sSfL -o "$TMP" "$URL"

mkdir -p "$(dirname "$OUT")"
python3 - "$TMP" "$OUT" <<'EOF'
import csv, re, sys

src, dst = sys.argv[1], sys.argv[2]
table = {}
with open(src, newline="", encoding="utf-8") as f:
    for row in csv.DictReader(f):
        prefix = row["Assignment"].strip().upper()
        name = re.sub(r"\s+", " ", row["Organization Name"]).strip()
        if re.fullmatch(r"[0-9A-F]{6}", prefix) and name:
            table.setdefault(prefix, name)
with open(dst, "w", encoding="utf-8") as f:
    for prefix in sorted(table):
        f.write(f"{prefix}\t{table[prefix]}\n")
print(f">> wrote {len(table)} prefixes to {dst}")
EOF

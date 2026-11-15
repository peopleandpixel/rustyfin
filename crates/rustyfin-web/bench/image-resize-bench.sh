#!/bin/sh
# Times the server's image renditions: requests the Primary image of up to N movies at one size,
# one at a time and then C at a time, and prints the per-request time and the throughput. Run it
# against a server whose disk image cache (<data>/cache/images) is empty and that was restarted,
# so that every request is a resize; run it again for the cached times.
#
# Usage: RUSTYFIN_URL=http://127.0.0.1:8096 RUSTYFIN_TOKEN=... \
#        image-resize-bench.sh [WIDTH=300] [N=60] [C=6]
# The token is read from the environment and never printed.
set -eu
URL=${RUSTYFIN_URL:?set RUSTYFIN_URL}
: "${RUSTYFIN_TOKEN:?set RUSTYFIN_TOKEN}"
WIDTH=${1:-300}
N=${2:-60}
C=${3:-6}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# The movies with a Primary image, as "id tag" lines, by name so that two runs ask for the same ones
curl -sf -H "X-Emby-Token: $RUSTYFIN_TOKEN" \
    "$URL/Items?Recursive=true&IncludeItemTypes=Movie&SortBy=SortName&Limit=$((N * 2 + 10))" \
    | python3 -I -c '
import json, sys
items = [i for i in json.load(sys.stdin).get("Items", []) if i.get("ImageTags", {}).get("Primary")]
for i in items:
    print(i["Id"], i["ImageTags"]["Primary"])
' > "$TMP/items"
[ -s "$TMP/items" ] || { echo "no movies with a Primary image"; exit 1; }

fetch() { # id tag -> "seconds http_code bytes"
    curl -s -o /dev/null -w '%{time_total} %{http_code} %{size_download}\n' \
        -H "X-Emby-Token: $RUSTYFIN_TOKEN" "$URL/Items/$1/Images/Primary?maxWidth=$WIDTH&tag=$2"
}
summary() { # label start end < "seconds code bytes" lines
    python3 -I -c '
import sys
label, wall = sys.argv[1], float(sys.argv[3]) - float(sys.argv[2])
rows = [l.split() for l in sys.stdin if l.strip()]
t = sorted(float(r[0]) * 1000 for r in rows)
bad = sum(1 for r in rows if r[1] != "200")
kb = sum(int(r[2]) for r in rows) / 1024 / max(1, len(rows))
p = lambda q: t[min(len(t) - 1, int((len(t) - 1) * q))]
print(f"{label}: {len(t)} images, p50 {p(.5):.0f} ms, p95 {p(.95):.0f} ms, max {t[-1]:.0f} ms, "
      f"{len(t) / wall:.1f} images/s, {kb:.0f} KB each, non-200 {bad}")
' "$1" "$2" "$3"
}

head -n "$N" "$TMP/items" > "$TMP/serial"
tail -n +"$((N + 1))" "$TMP/items" | head -n "$N" > "$TMP/parallel"
export URL WIDTH RUSTYFIN_TOKEN
t0=$(date +%s.%N)
while read -r id tag; do fetch "$id" "$tag"; done < "$TMP/serial" > "$TMP/serial.out"
t1=$(date +%s.%N)
summary "width $WIDTH, one at a time" "$t0" "$t1" < "$TMP/serial.out"
if [ -s "$TMP/parallel" ]; then
    t0=$(date +%s.%N)
    xargs -P "$C" -L 1 sh -c 'curl -s -o /dev/null -w "%{time_total} %{http_code} %{size_download}\n" \
        -H "X-Emby-Token: $RUSTYFIN_TOKEN" "$URL/Items/$0/Images/Primary?maxWidth=$WIDTH&tag=$1"' \
        < "$TMP/parallel" > "$TMP/parallel.out"
    t1=$(date +%s.%N)
    summary "width $WIDTH, $C at a time" "$t0" "$t1" < "$TMP/parallel.out"
fi

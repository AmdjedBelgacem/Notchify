#!/usr/bin/env bash
# Build AppIcon.icns from the 1024 px master (macOS: sips + iconutil).
#   packaging/make_icns.sh packaging/icon/AppIcon-1024.png out/AppIcon.icns
set -euo pipefail
SRC="$1"; OUT="$2"
SET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$SET" "$(dirname "$OUT")"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SRC" --out "$SET/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$SRC" --out "$SET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$OUT"

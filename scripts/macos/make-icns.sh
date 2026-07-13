#!/usr/bin/env bash
#
# make-icns.sh — build a macOS .icns from a single square PNG.
#
# The brand icon lives at frontend/public/pwa-512x512.png (512x512 RGBA); this
# turns it into the multi-resolution .icns that Pulp.app references via
# CFBundleIconFile. Kept as a standalone script (not inline YAML) so it is
# reviewable and shellcheck-able. Requires the macOS `sips` + `iconutil` tools.
#
# Usage: make-icns.sh <input.png> <output.icns>
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <input-png> <output-icns>" >&2
  exit 2
fi

SRC="$1"
OUT="$2"

if [ ! -f "$SRC" ]; then
  echo "error: source PNG not found: $SRC" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ICONSET="$WORK/pulp.iconset"
mkdir -p "$ICONSET"

# Apple's .iconset expects each logical size in a 1x and a 2x (Retina) variant.
# 512@2x is 1024px; sips upscales the 512px source for that one slot.
for size in 16 32 128 256 512; do
  double=$(( size * 2 ))
  sips -z "$size" "$size"     "$SRC" --out "$ICONSET/icon_${size}x${size}.png"     >/dev/null
  sips -z "$double" "$double" "$SRC" --out "$ICONSET/icon_${size}x${size}@2x.png"  >/dev/null
done

iconutil --convert icns "$ICONSET" --output "$OUT"
echo "wrote $OUT"

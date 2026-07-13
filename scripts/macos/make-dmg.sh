#!/usr/bin/env bash
#
# make-dmg.sh — package a Pulp.app into a drag-to-install .dmg.
#
# Stages the bundle next to an /Applications symlink (the standard
# drag-into-Applications layout) and compresses it into a UDZO .dmg. Kept as a
# standalone script — matching make-app.sh / make-icns.sh — so both the release
# workflow (.github/workflows/release-macos-app.yml) and the local build
# (build-local.sh) produce the .dmg the exact same way. Requires macOS `hdiutil`.
#
# Signing/notarization is the caller's job (the CI signs the .dmg after this);
# this script only assembles it.
#
# Usage: make-dmg.sh <Pulp.app> <output.dmg>
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <Pulp.app> <output.dmg>" >&2
  exit 2
fi

APP="$1"
DMG="$2"

if [ ! -d "$APP" ]; then
  echo "error: not an app bundle: $APP" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/$(basename "$APP")"
# Drag-to-install: an /Applications symlink next to the app.
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create -volname "Pulp" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
echo "wrote $DMG"

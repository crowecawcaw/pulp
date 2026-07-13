#!/usr/bin/env bash
#
# build-local.sh — build an UNSIGNED universal Pulp.app + .dmg on your own Mac,
# for testing the desktop app without the CI signing/notarization pipeline.
#
# Mirrors .github/workflows/release-macos-app.yml minus the Apple signing steps,
# reusing the same make-icns.sh / make-app.sh / make-dmg.sh building blocks so
# the local artifact is assembled identically to the released one. The result is
# UNSIGNED: it runs fine on THIS Mac, but Gatekeeper will warn on other machines
# (right-click → Open to bypass). For a distributable signed+notarized .dmg, use
# the release workflow with the Apple secrets configured.
#
# Output goes to backend/target/macos-app/ (already gitignored).
#
# Usage:  scripts/macos/build-local.sh
# Env:    PULP_BUNDLE_ID   reverse-DNS bundle id (passed through to make-app.sh)
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

VERSION="$(grep -m1 '^version' backend/Cargo.toml | cut -d'"' -f2)"
OUT="$REPO/backend/target/macos-app"
mkdir -p "$OUT"

# The web UI is embedded into the release binary from backend/web-dist via
# rust-embed; rebuild it so the app doesn't ship a stale (or empty) UI.
echo "==> Building frontend (backend/web-dist, embedded at compile time)…"
if [ -d frontend/node_modules ]; then
  ( cd frontend && npm run build )
else
  ( cd frontend && npm ci && npm run build )
fi

echo "==> Building tray binary for both arches (release)…"
( cd backend
  cargo build --release --features tray --target aarch64-apple-darwin
  cargo build --release --features tray --target x86_64-apple-darwin )

echo "==> Creating universal binary (lipo)…"
lipo -create \
  backend/target/aarch64-apple-darwin/release/pulp \
  backend/target/x86_64-apple-darwin/release/pulp \
  -output "$OUT/pulp-universal"
lipo -info "$OUT/pulp-universal"

echo "==> Generating .icns from the brand PNG…"
bash scripts/macos/make-icns.sh frontend/public/pwa-512x512.png "$OUT/pulp.icns"

echo "==> Assembling Pulp.app…"
bash scripts/macos/make-app.sh "$OUT/pulp-universal" "$OUT/pulp.icns" "$VERSION" "$OUT/Pulp.app"

echo "==> Building .dmg…"
DMG="$OUT/Pulp-${VERSION}-universal.dmg"
bash scripts/macos/make-dmg.sh "$OUT/Pulp.app" "$DMG"

echo
echo "Done — UNSIGNED (Gatekeeper will warn on other Macs; fine locally):"
echo "  app: $OUT/Pulp.app"
echo "  dmg: $DMG"
echo
echo "Test it:   open \"$OUT/Pulp.app\"   (look for the icon in the menu bar)"

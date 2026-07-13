#!/usr/bin/env bash
#
# make-app.sh — assemble Pulp.app around a (universal) `pulp` binary.
#
# Layout produced:
#   Pulp.app/Contents/
#     Info.plist              (LSUIElement menu-bar app; CFBundleExecutable=pulp)
#     MacOS/pulp              the real universal binary — the bundle entry point
#     Resources/pulp.icns     brand icon
#
# The binary IS the bundle's CFBundleExecutable. When macOS launches it with no
# arguments (a Finder double-click / `open`), `pulp` detects it's running from
# inside a .app bundle and defaults to `pulp app` — the system-tray / menu-bar
# launcher (see `launched_from_app_bundle` in backend/src/main.rs) — NOT the bare
# `pulp serve`.
#
# We deliberately do NOT ship a separate shell-script launcher as
# CFBundleExecutable: on macOS's default case-INSENSITIVE filesystem a launcher
# named `Pulp` and the `pulp` binary resolve to the SAME path, so one clobbers
# the other (the app ends up with no real binary). Keeping the Mach-O binary as
# the single entry point avoids that entirely and is also the shape Apple's
# hardened-runtime notarization expects.
#
# LSUIElement=true makes it an accessory (menu-bar) app with no Dock icon, the
# correct behaviour for a tray app.
#
# Usage: make-app.sh <pulp-binary> <pulp.icns> <version> <output.app>
# Env:   PULP_BUNDLE_ID  reverse-DNS bundle id (default com.nimbuslabs.pulp).
#                        Maintainers should set their own owned identifier.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <pulp-binary> <icns> <version> <output.app>" >&2
  exit 2
fi

BIN="$1"
ICNS="$2"
VERSION="$3"
APP="$4"
BUNDLE_ID="${PULP_BUNDLE_ID:-com.nimbuslabs.pulp}"

for f in "$BIN" "$ICNS"; do
  if [ ! -f "$f" ]; then
    echo "error: missing input: $f" >&2
    exit 1
  fi
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# The real (universal) binary — the bundle's entry point.
cp "$BIN" "$APP/Contents/MacOS/pulp"
chmod +x "$APP/Contents/MacOS/pulp"

# Brand icon.
cp "$ICNS" "$APP/Contents/Resources/pulp.icns"

# Info.plist — unquoted heredoc so $BUNDLE_ID / $VERSION expand.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Pulp</string>
	<key>CFBundleDisplayName</key>
	<string>Pulp</string>
	<key>CFBundleIdentifier</key>
	<string>${BUNDLE_ID}</string>
	<key>CFBundleExecutable</key>
	<string>pulp</string>
	<key>CFBundleIconFile</key>
	<string>pulp.icns</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${VERSION}</string>
	<key>CFBundleVersion</key>
	<string>${VERSION}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<!-- Menu-bar accessory app: no Dock icon, correct for a tray launcher. -->
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

echo "wrote $APP (bundle id: $BUNDLE_ID, version: $VERSION)"

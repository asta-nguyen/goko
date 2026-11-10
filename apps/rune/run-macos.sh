#!/usr/bin/env bash
# Build the Rune app and launch it as a macOS .app bundle.
#
# A bare binary opens no window on macOS: without a bundle, LaunchServices
# never activates the app, so its window is never ordered in (the process
# sits in the event loop with nothing on screen). The bundle gives the app
# an identity (dev.goko.rune), a Dock icon and a normal activation.
#
# Usage: apps/rune/run-macos.sh [release|debug]
set -euo pipefail
cd "$(dirname "$0")/../.."

PROFILE="${1:-release}"
if [ "$PROFILE" = "release" ]; then
  cargo build --workspace --release
  BIN_DIR="target/release"
else
  cargo build --workspace
  BIN_DIR="target/debug"
fi

APP="target/Goko.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN_DIR/goko-rune" "$APP/Contents/MacOS/goko-rune"
# The daemon lives beside the app: `daemon_bin()` looks for `gokod` next to
# the running executable, so the app can start it when no daemon answers.
cp "$BIN_DIR/gokod" "$APP/Contents/MacOS/gokod"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key>
	<string>Goko</string>
	<key>CFBundleExecutable</key>
	<string>goko-rune</string>
	<key>CFBundleIdentifier</key>
	<string>dev.goko.rune</string>
	<key>CFBundleName</key>
	<string>Goko</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleVersion</key>
	<string>0.1.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

# The running instance is replaced: `open` would only bring the old one to
# the front, leaving the previous build on screen.
pkill -f "Goko.app/Contents/MacOS/goko-rune" 2>/dev/null || true
sleep 1

open "$APP"
echo "launched $APP"
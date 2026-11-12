#!/usr/bin/env bash
# Assemble the Goko.app bundle from built binaries and sign it ad hoc.
#
# Usage: apps/rune/bundle-macos.sh <bin-dir> <app-path> [version]
#
# `version` defaults to the workspace version in Cargo.toml.
set -euo pipefail

BIN_DIR="${1:?bin dir (target/release or target/debug)}"
APP="${2:?path of the Goko.app to create}"
VERSION="${3:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$0")/../../Cargo.toml" | head -1)}"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$(dirname "$0")/assets/Goko.icns" "$APP/Contents/Resources/Goko.icns"
cp "$BIN_DIR/goko-rune" "$APP/Contents/MacOS/goko-rune"
# The daemon lives beside the app: `daemon_bin()` looks for `gokod` next to
# the running executable, so the app can start it when no daemon answers.
cp "$BIN_DIR/gokod" "$APP/Contents/MacOS/gokod"

cat > "$APP/Contents/Info.plist" <<PLIST
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
	<key>CFBundleIconFile</key>
	<string>Goko.icns</string>
	<key>CFBundleName</key>
	<string>Goko</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

# Ad hoc: Apple Silicon refuses unsigned code, and this seals the bundle. It is
# free and carries no identity; a certificate would replace the "-".
codesign --force --deep --sign - "$APP"

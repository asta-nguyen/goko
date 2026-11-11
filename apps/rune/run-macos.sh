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
apps/rune/bundle-macos.sh "$BIN_DIR" "$APP"

# The running instance is replaced: `open` would only bring the old one to
# the front, leaving the previous build on screen.
pkill -f "Goko.app/Contents/MacOS/goko-rune" 2>/dev/null || true
sleep 1

open "$APP"
echo "launched $APP"
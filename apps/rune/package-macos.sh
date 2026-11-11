#!/usr/bin/env bash
# Build a release Goko.app and pack it for a GitHub release.
#
# Usage: apps/rune/package-macos.sh <version> <download-base-url>
#
# Writes dist/Goko-<version>-macos-aarch64.tar.gz and dist/macos-aarch64.target.json,
# the entry for latest.json (its url is <download-base-url>/<archive>).
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION="${1:?version, e.g. 0.1.0}"
BASE="${2:?base url the archive will be served from}"

[ "$(uname -m)" = arm64 ] || { echo "this builds an Apple Silicon bundle: run it on arm64" >&2; exit 1; }

cargo build --workspace --release

rm -rf dist
mkdir dist
apps/rune/bundle-macos.sh target/release dist/Goko.app "$VERSION"
codesign --verify --deep --strict dist/Goko.app

FILE="Goko-$VERSION-macos-aarch64.tar.gz"
tar -czf "dist/$FILE" -C dist Goko.app
rm -rf dist/Goko.app
SHA="$(shasum -a 256 "dist/$FILE" | cut -d' ' -f1)"

# One entry of latest.json; the workflow merges every platform's.
printf '{"macos-aarch64":{"url":"%s/%s","sha256":"%s"}}\n' "$BASE" "$FILE" "$SHA" > dist/macos-aarch64.target.json

echo "packed dist/$FILE ($SHA)"

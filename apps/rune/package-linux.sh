#!/usr/bin/env bash
# Build a release Goko for Linux and pack it for a GitHub release.
#
# Usage: apps/rune/package-linux.sh <version> <download-base-url>
#
# Writes dist/Goko-<version>-linux-<arch>.tar.gz (a `goko/` folder holding
# `goko-rune` and `gokod`, which the app looks for beside itself) and
# dist/linux-<arch>.target.json, the entry for latest.json.
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION="${1:?version, e.g. 0.1.0}"
BASE="${2:?base url the archive will be served from}"

case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) echo "no Linux build for $(uname -m)" >&2; exit 1 ;;
esac
TARGET="linux-$ARCH"

cargo build --workspace --release

rm -rf dist
mkdir -p dist/goko
cp target/release/goko-rune target/release/gokod dist/goko/

FILE="Goko-$VERSION-$TARGET.tar.gz"
tar -czf "dist/$FILE" -C dist goko
rm -rf dist/goko
SHA="$(sha256sum "dist/$FILE" | cut -d' ' -f1)"

printf '{"%s":{"url":"%s/%s","sha256":"%s"}}\n' "$TARGET" "$BASE" "$FILE" "$SHA" > "dist/$TARGET.target.json"

echo "packed dist/$FILE ($SHA)"

#!/usr/bin/env bash
# Regenerate committed icon exports from icon.svg with macOS tools.
set -euo pipefail
cd "$(dirname "$0")"

swift - <<'SWIFT'
import AppKit

guard let image = NSImage(contentsOfFile: "icon.svg"),
      let tiff = image.tiffRepresentation,
      let bitmap = NSBitmapImageRep(data: tiff),
      let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("Cannot render icon.svg")
}
assert(bitmap.pixelsWide == 1024 && bitmap.pixelsHigh == 1024)
try png.write(to: URL(fileURLWithPath: "icon.png"))
SWIFT

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/Goko.iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" icon.png --out "$work/Goko.iconset/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" icon.png --out "$work/Goko.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work/Goko.iconset" -o Goko.icns

# App icons

## Requirements

### ICO-packaged-icon

The system SHALL include the Goko icon in macOS development and release bundles
and Linux release archives, with the Linux installer selecting the packaged PNG
when present and the generic terminal icon for older archives without it.

#### Scenario: macOS bundle

- GIVEN the app and daemon binaries have been built
- WHEN the shared bundle script assembles the app
- THEN it copies `Goko.icns` into Resources, declares it in Info.plist, and signs the bundle

#### Scenario: Linux installation

- GIVEN a Linux release archive contains `icon.png`
- WHEN the installer installs the archive
- THEN the desktop launcher references the installed PNG by its absolute path

#### Scenario: older Linux release

- GIVEN an older Linux archive has no icon
- WHEN the installer installs the archive
- THEN its desktop launcher uses `utilities-terminal`

Evidence: `apps/rune/bundle-macos.sh`, `apps/rune/run-macos.sh`, `apps/rune/package-macos.sh`, `apps/rune/package-linux.sh`, `distribution/install.sh`, `distribution/test-icons.py`

## Flow

The editable SVG depicts a G made of ivory and orange pieces on charcoal.
The README displays the SVG. The macOS-only asset generator renders a 1024 px
PNG through AppKit and builds the multi-size ICNS using `sips` and `iconutil`.
Both exports are committed; normal builds only copy them.

Both macOS run and release scripts call the shared bundle script before launch
or archiving. Linux packaging places the PNG beside the binaries; installation
keeps those files together and writes the launcher entry.

## State changes

Only generated asset files, app bundles, release archives, and the installed
Linux launcher change. Project and session data are unaffected.

## Side effects

macOS bundle assembly signs the app ad hoc. Linux installation replaces the
installed app directory and refreshes the desktop database when available.

## Authorization & constraints

Regenerating the exports requires macOS and Swift. Packaging uses the checked-in
exports and needs no graphics converter.

## Error paths

Asset generation fails if SVG rendering fails. The packaging scripts stop on
missing icon exports. Missing icons in older Linux archives retain the fallback.

## Tests

`python3 distribution/test-icons.py` packages stub Linux binaries in a temporary
directory and runs the real installer with local file URLs, checking the icon
bytes, launcher path, and older-archive fallback without touching real installs.
macOS verification assembles a bundle, checks its plist and icon resource, and
runs `codesign --verify --deep --strict`.

## Related

- [Architecture overview](../architecture/overview.md)

## Sources

- `apps/rune/assets/icon.svg`
- `apps/rune/assets/generate-macos.sh`
- `apps/rune/bundle-macos.sh`
- `apps/rune/run-macos.sh`
- `apps/rune/package-macos.sh`
- `apps/rune/package-linux.sh`
- `distribution/install.sh`
- `distribution/test-icons.py`
- `README.md`

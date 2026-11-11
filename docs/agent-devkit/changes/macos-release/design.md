# Release: a prebuilt app for macOS and Linux, installed with a script

## Goal

Someone can install Goko on an Apple Silicon Mac or a Linux machine (x86_64 or
aarch64) with one command, without building it, and without a paid Apple
Developer account.

## Approach

CI builds the app once per release tag; users download the result.

- A tag `vX.Y.Z` triggers `.github/workflows/release.yml`, which builds on
  `macos-15`, `ubuntu-22.04` and `ubuntu-22.04-arm` (22.04 keeps the glibc
  requirement low). Each runs its packaging script: `apps/rune/package-macos.sh`
  builds the workspace in release mode, assembles `Goko.app` (`gokod` beside the
  app, as `run-macos.sh` does) and signs it ad hoc; `package-linux.sh` packs a
  `goko/` folder with `goko-rune` and `gokod`. Each writes
  `dist/Goko-<version>-<target>.tar.gz` and `dist/<target>.target.json`, one
  entry of the manifest.
- A publish job merges the entries with `jq` into `latest.json`
  (`{"version", "targets": {"<target>": {"url", "sha256"}}}`, targets
  `macos-aarch64`, `linux-x86_64`, `linux-aarch64`) and publishes the archives,
  `latest.json` and `distribution/install.sh` as a GitHub release.
- `distribution/install.sh` (run as `curl -fsSL
  https://github.com/asta-nguyen/goko/releases/latest/download/install.sh | sh`)
  reads `latest.json`, downloads the archive for this machine, checks its
  sha256, and installs. macOS: verifies the bundle's signature and replaces
  `~/Applications/Goko.app`. Linux: replaces `~/.local/share/goko`, links
  `~/.local/bin/goko` to it and writes a `.desktop` entry. Running it again
  updates.

`run-macos.sh` and the packaging share one bundle script
(`apps/rune/bundle-macos.sh`), so the dev bundle and the release bundle cannot
drift.

## Update notice

The app looks for a newer release itself (`apps/rune/src/update.rs`). It reads
the same `latest.json` the installer uses, at startup, every six hours and on
"Check for updates", using `curl` (no new dependency). Settings → Updates shows
the running version, what the check found, and a "Copy install command" button
when a newer release with a build for this machine is out; a notice toast says so once per version. A failed
automatic check stays silent; one the user asked for shows its reason.
`GOKO_NO_UPDATE_CHECK=1` stops the automatic check and `GOKO_RELEASES_URL`
points it elsewhere (as for the installer). The app does not download or swap
itself: updating is running the installer.

## Signing

Ad hoc (`codesign --sign -`): free, no Apple account. Apple Silicon refuses
unsigned code, and the signature seals the bundle. A download through `curl` is
not quarantined, so Gatekeeper does not prompt; a DMG opened from a browser would.
No notarization. The ad hoc identity changes with every build, so macOS may ask
again for permissions after an update.

## Limits and out of scope

- Apple Silicon only on macOS; Intel builds from source. No Windows release:
  `gokod` uses Unix sockets and `libc`. Linux needs a desktop session and a GPU
  driver; the Linux build has not been run yet (only `protocol` and `gokod` were
  checked for the Linux target locally), so the first tag may need build
  dependencies adjusted.
- No in-app updater that downloads and swaps the app (a later change: download
  beside the running app and swap on relaunch). Updating is re-running the
  installer.
- An older `gokod` that is already running keeps serving until it exits; the
  installer says so and never stops it.
- No tests in CI (the build is the gate).
- A DMG, notarization and a Developer ID are only worth it with many users.

## Verification

Run `package-macos.sh` locally, merge the entry with `jq`, then `install.sh`
against the local `dist/` (`GOKO_RELEASES_URL=file://…`, a temporary `HOME`):
the app installs, its signature verifies, and a corrupted archive is refused.
The Linux branch is run the same way with a stub `uname` and a hand-made
archive; the real Linux build only runs in CI. The workflow itself can
only be proven by pushing a tag; `workflow_dispatch` runs the same build and
keeps the archive as a workflow artifact without publishing.

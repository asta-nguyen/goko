# Release tasks (macOS and Linux)

## Design

[Release: a prebuilt app for macOS and Linux, installed with a script](design.md)

## Impact map

Entry: `apps/rune/run-macos.sh` (bundle assembly), new release workflow and scripts.
Flow: tag → CI → `package-macos.sh` → GitHub release assets → `install.sh` → `~/Applications/Goko.app`.
State changes: none in the app; the installer writes `~/Applications/Goko.app` only.
External effects: a GitHub release per tag; no secrets beyond the workflow token.
Verification: local package + install run described in the design.

## Tasks

1. `apps/rune/bundle-macos.sh` (assemble + ad hoc sign, version from the argument or `Cargo.toml`); `run-macos.sh` calls it. Verify: `run-macos.sh debug` bundle verifies with `codesign --verify --deep --strict`.
2. `apps/rune/package-macos.sh` (release build, archive, `latest.json`). Verify: the archive and manifest exist and the sha256 matches.
3. `distribution/install.sh`. Verify: installs from a local `dist/` into a temporary `HOME`; refuses a corrupted archive.
4. `.github/workflows/release.yml`. Verify: `actionlint`-style read-through; proven by a tag push.
5. README install section; `.gitignore` `/dist/`.
6. Linux: `apps/rune/package-linux.sh`, an `install.sh` Linux branch, a matrix build plus a publish job that merges the `<target>.target.json` entries with `jq`, and a platform-aware update check (`update::target`). Verify: install from a stub-`uname` Linux run; `cargo check --target x86_64-unknown-linux-gnu -p protocol -p daemon`.
7. Update notice: `apps/rune/src/update.rs` (version compare, `latest.json` check, background loop) and Settings → Updates in `settings.rs`; wiki `RNC-settings-page`; `CHANGELOG.md`. Verify: `cargo test -p goko-rune update`.

## Approval Gate

Status: approved — the user chose this approach in chat (ad hoc signing, a `curl` installer with sha256, GitHub Releases) and asked to set it up.

## Results

- Task 1: `bundle-macos.sh` builds and signs the bundle (`Signature=adhoc`), version from `Cargo.toml` by default (`CFBundleVersion` 0.1.0); `run-macos.sh` calls it (syntax-checked, not launched).
- Task 2: `package-macos.sh 0.1.0 file://…/dist` packed `Goko-0.1.0-macos-aarch64.tar.gz` and a `latest.json` whose sha256 matches.
- Task 3: `install.sh` against the local `dist/` with a temporary `HOME` installed `Goko.app` (signature verified, `gokod 0.1.3` inside); a corrupted archive stopped with "checksum mismatch" and installed nothing; an unreachable URL stopped with "cannot reach".
- Task 4: the workflow parses as YAML and has the expected steps; it has not run — only a tag push proves it.
- Task 5: README install section and `.gitignore` done.
- Task 7: `cargo test -p goko-rune update` — version compare (numeric, pre-release older), manifest parse, a `file://` manifest checked (newer reported, same version and missing file not), notice once per release, failure shown only when asked, and the Settings → Updates view shows the running version and the copy button only when a release is out.
- Task 6 (Linux): `cargo check --target x86_64-unknown-linux-gnu -p protocol -p daemon` passes; `app-core` and `goko-rune` cannot be checked for Linux from macOS (a C cross-compiler is missing for bundled C dependencies), so only CI proves them. `install.sh` with a stub `uname`, a hand-made archive and a `jq`-merged manifest installed `~/.local/share/goko`, the `goko` symlink and the `.desktop` entry; the macOS path was re-run after the manifest change. `update::check` now reports a release only if its manifest has a build for this machine (`update::target`).
- Not done: no tag was pushed and nothing was published; the app was not opened to look at the new section.

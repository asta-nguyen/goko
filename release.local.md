# Releasing Goko by hand

The flow used for v0.1.1 (and v0.1.0): build on this Mac, publish with `gh`.
Use it when the `release` workflow cannot get a macOS runner (it queued for 5+
minutes on "capacity constraints" and was cancelled). The workflow builds macOS
and Linux (x86_64, aarch64); by hand only this Mac and an arm64 container are
available. Replace `0.1.1` with the new version.

## 1. Prepare

1. Set `version` in `[workspace.package]` of `Cargo.toml`; `cargo build` refreshes
   `Cargo.lock`. `gokod` keeps its own version (`crates/daemon/Cargo.toml`); bump
   it only when the app gates on a new daemon feature (`version_at_least` in
   `apps/rune/src/daemon.rs`).
2. `CHANGELOG.md`: put the pending entries under `## [0.1.1] - YYYY-MM-DD` and keep
   an empty `## [Unreleased]` above. List failing checks under `### Known issues`.
3. `cargo fmt --all -- --check` and `cargo test --workspace`. v0.1.1 shipped with
   `rune-kit` `an_open_span_never_shows_its_markers` failing (it only runs with the
   `markdown` feature, so `-p rune-kit` alone passes).
4. Commit, merge to `main`, `git push origin main`.
5. `git tag v0.1.1 && git push origin v0.1.1`. Never move a tag that was pushed.

The tag push starts the `release` workflow. If you publish by hand, cancel that run
first (`gh run list --workflow release`, then `gh run cancel <id>`), or its
`gh release create` will collide with yours.

## 2. Build

Build from the tagged commit: `for r in 'v0.1.1^{commit}' HEAD origin/main; do git rev-parse --short "$r"; done`
must print one hash three times. Both package scripts `rm -rf dist`, so keep each
platform's `dist/` separate.

**macOS** (about a minute when warm), writes `dist/`:

```sh
apps/rune/package-macos.sh 0.1.1 https://github.com/asta-nguyen/goko/releases/download/v0.1.1
```

**Linux aarch64**, in an OrbStack Ubuntu 22.04 container (about 3 minutes). Build
from a clean copy of `HEAD` so the container does not overwrite the Mac's
`target/` and `dist/`:

```sh
S=$(mktemp -d); git archive HEAD | tar -x -C $S
cat > $S/.linux-build.sh <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get -o Acquire::Retries=5 update -qq
apt-get -o Acquire::Retries=5 install -y -qq --no-install-recommends build-essential pkg-config curl ca-certificates git jq \
  libfontconfig1-dev libwayland-dev libxkbcommon-dev libx11-dev libxcb1-dev libxkbcommon-x11-dev libvulkan-dev
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
. "$HOME/.cargo/env"
cd /work
apps/rune/package-linux.sh 0.1.1 https://github.com/asta-nguyen/goko/releases/download/v0.1.1
EOF
docker run --rm --platform linux/arm64 -v $S:/work ubuntu:22.04 bash /work/.linux-build.sh
cp $S/dist/Goko-0.1.1-linux-aarch64.tar.gz $S/dist/linux-aarch64.target.json dist/
```

The script must be inside the mounted directory (`/work`), not beside it. Linux
x86_64 is not built: it needs an x86 machine (emulation is very slow).

## 3. Check and publish

```sh
# each archive's sha256 must equal the one in its target.json
shasum -a 256 dist/*.tar.gz; cat dist/*.target.json
# one manifest for every platform; the version comes first, as install.sh reads it
jq -cs --arg version 0.1.1 '{version: $version, targets: add}' dist/*.target.json > dist/latest.json
gh release create v0.1.1 dist/*.tar.gz dist/latest.json distribution/install.sh \
  --title "Goko 0.1.1" --generate-notes
```

This publishes publicly: `latest.json` makes running apps report a newer version.
Leave a platform out by deleting its archive and target.json from `dist/` first.

## 4. Verify

```sh
gh release view v0.1.1                       # 4 assets: 2 tar.gz, latest.json, install.sh
gh release download v0.1.1 -p latest.json -O -
gh release download v0.1.1 -p 'Goko-0.1.1-macos-aarch64.tar.gz' -O - | shasum -a 256
# the installer on a clean Linux, end to end
docker run --rm --platform linux/arm64 ubuntu:22.04 bash -c '
  apt-get -qq update >/dev/null && apt-get -qq install -y curl ca-certificates >/dev/null
  curl -fsSL https://github.com/asta-nguyen/goko/releases/latest/download/install.sh | sh
  ~/.local/share/goko/gokod --version'
```

A clean Ubuntu lacks `libxkbcommon.so.0` (`ldd ~/.local/share/goko/goko-rune`); the
app itself was not run on a Linux desktop.

## Known limits

- macOS is signed ad hoc, not notarized; Linux has no signature.
- Hand-built archives do not reproduce CI's: their sha256 differs from a CI build.
- Before the first tag that relies on Linux in CI, run the workflow by hand (it
  keeps archives as artifacts, no publish) and check all three archives.

# Goko

Goko is a desktop app for macOS and Linux (Windows is not supported yet) for
running interactive command-line agents by project. The native Rune app
(`apps/rune`) talks to the Rust `gokod` daemon, which owns the terminal
processes. Project and session metadata is stored separately in SQLite. See
[ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

## Layout

| Path | What it is |
|---|---|
| `apps/rune` | The native app (Cargo package `goko-rune`): UI, terminal, editor |
| `crates/daemon` | `gokod`, the daemon that owns every PTY and child process |
| `crates/protocol` | Wire messages and framing shared by the app and `gokod` |
| `crates/app-core` | `goko.db` store, Git/worktrees, agents, ACP, language servers |

## Install (macOS Apple Silicon)

```sh
curl -fsSL https://github.com/asta-nguyen/goko/releases/latest/download/install.sh | sh
```

This downloads the latest release, checks its sha256 and signature, and installs
`Goko.app` in `~/Applications`; run it again to update. The app is signed ad hoc
(free, no Apple Developer account), so install it with this command: a copy
downloaded through a browser is quarantined and macOS asks you to allow it in
System Settings → Privacy & Security.

Intel Macs and Linux build from source (below): the installer and
`apps/rune/package-linux.sh` already handle Linux, but the release workflow does
not build it yet. There is no Windows release.

Goko → Settings → Updates shows the running version and tells you when a newer
release is out (set `GOKO_NO_UPDATE_CHECK=1` to stop the automatic check). To
update, run the install command again and reopen Goko.

## Prerequisites

- Rust stable, installed with [rustup](https://rustup.rs/).
- macOS: Xcode Command Line Tools (`xcode-select --install`).
- Linux: a C toolchain, `git`, `pkg-config`, and the font, X11/Wayland and GPU
  libraries that `winit`/`wgpu` need. On Debian or Ubuntu:
  `sudo apt-get install build-essential pkg-config libfontconfig1-dev libwayland-dev
  libxkbcommon-dev libx11-dev libxcb1-dev libxkbcommon-x11-dev libvulkan-dev`.
- Windows: not supported yet (see below).

No Node, pnpm or other toolchain is needed.

## Build

```sh
cargo build --workspace            # debug
cargo build --workspace --release  # release
```

Binaries land in `target/{debug,release}/`: `goko-rune` (the app) and `gokod`
(the daemon).

## Run locally

The app starts `gokod` itself when no daemon answers on the socket, so you never
start the daemon by hand (to run it alone: `cargo run -p daemon`; `gokod --version`
prints its version, and a second instance prints `gokod: already running` and
exits). It looks for `gokod` beside its own executable (override
with `GOKO_DAEMON_BIN`), so build the whole workspace first.

### macOS

A bare binary opens no window on macOS, so `cargo run -p goko-rune` is not enough.
Use the bundle script: it builds the workspace, assembles `target/Goko.app` with
`gokod` beside it, kills any running instance, and launches the new build.

```sh
apps/rune/run-macos.sh          # release build (default)
apps/rune/run-macos.sh debug    # faster build, slower app
```

### Linux

```sh
cargo build --workspace --release
target/release/goko-rune
```

### Windows

Not buildable yet: the daemon and its client currently use Unix domain sockets,
`flock` and process groups (`killpg`), so the workspace only compiles on macOS and
Linux. Windows needs a port of those parts (named pipe or TCP transport, a lock
file, job objects) before it can run.

### Data and logs

App data lives in `$GOKO_HOME`, default `~/Library/Application Support/Goko`
(on every platform for now: the default is not yet per-OS, so set `GOKO_HOME` on
Linux if you want an XDG path): `goko.db`, `daemon.sock`, `daemon.token`, `daemon.lock` and `daemon.log`.

Keep development data separate from your normal data with a throwaway home:

```sh
GOKO_HOME=/tmp/goko-dev apps/rune/run-macos.sh   # Linux: GOKO_HOME=/tmp/goko-dev target/release/goko-rune
```

A fresh home gets its own daemon, so it never touches sessions in your real one.
To reset everything, quit Goko and delete that directory.

## Checks

```sh
cargo test --workspace                     # protocol, daemon and app tests
cargo fmt --check
cargo clippy --workspace -- -D warnings
```

The daemon integration tests start the real `gokod` in a temporary `GOKO_HOME`.

## Releasing

A release is a pushed tag. The app's version is the workspace `version` in
`Cargo.toml` (it is what Settings → Updates compares), so the tag must equal it:

1. Set `version` in `[workspace.package]` of `Cargo.toml` (for example `0.2.0`),
   and rename the `Unreleased` heading in `CHANGELOG.md` to that version and the
   date, leaving a fresh `Unreleased` above it.
2. Merge to `main`, then `git tag v0.2.0 && git push origin v0.2.0`.

`.github/workflows/release.yml` then builds macOS (Apple Silicon) and publishes
the archive, a `latest.json` manifest and `install.sh` as a GitHub release; the workflow refuses a tag that does not match
`Cargo.toml`. To try the builds without publishing, run the workflow by hand
(Actions → release → Run workflow): it keeps the archives as artifacts.

macOS builds are signed ad hoc (no Apple Developer account, so no
notarization), which is why the installer is a `curl` command; see the macOS
note above. The packaging scripts are `apps/rune/package-macos.sh` and
`apps/rune/package-linux.sh` (Linux is not in the workflow yet); `gokod` has its own version (`gokod --version`).

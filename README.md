# Goko

Goko is a cross-platform desktop app (macOS, Linux and Windows) for running
interactive command-line agents by project. The native Rune app (`apps/rune`) talks to the Rust `gokod` daemon, which
owns the terminal processes. Project and session metadata is stored separately in
SQLite. See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

## Layout

| Path | What it is |
|---|---|
| `apps/rune` | The native app (Cargo package `goko-rune`): UI, terminal, editor |
| `crates/daemon` | `gokod`, the daemon that owns every PTY and child process |
| `crates/protocol` | Wire messages and framing shared by the app and `gokod` |
| `crates/app-core` | `goko.db` store, Git/worktrees, agents, ACP, language servers |

## Prerequisites

- Rust stable, installed with [rustup](https://rustup.rs/).
- macOS: Xcode Command Line Tools (`xcode-select --install`).
- Linux: a C toolchain, `git`, and the X11/Wayland and GPU (Vulkan or OpenGL)
  libraries that `winit`/`wgpu` need.
- Windows: MSVC build tools and `git`.

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

## Distribution

There are no installers yet. On macOS, `target/Goko.app` is an unsigned local
bundle, fine for your own machine; sharing it needs Developer ID signing and
notarization first. On Linux, ship `goko-rune` and `gokod` together in one directory.

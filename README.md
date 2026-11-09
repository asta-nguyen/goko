# Goko

Goko is a macOS-first desktop app for running interactive command-line agents by
project. The Tauri/React app talks to the Rust `gokod` daemon, which owns the
terminal processes. Project and session metadata is stored separately in SQLite.

## macOS prerequisites

- macOS with Xcode Command Line Tools. Install them with `xcode-select --install`.
  Full Xcode is only needed if you later build the iOS target.
- Rust stable, installed with [rustup](https://rustup.rs/).
- Node.js 22.22.2 or newer in the 22.x line (or 24.15.0 or newer in the 24.x
  line) and pnpm 10. The current lockfile includes packages with these Node
  engine requirements.

Tauri's [macOS prerequisites](https://v2.tauri.app/start/prerequisites/) explain
the native toolchain requirements.

## Install dependencies

From the repository root:

```sh
pnpm install
```

## Run the macOS app in development

From the repository root:

```sh
pnpm -C apps/desktop tauri dev
```

This starts the native Tauri window. Its `beforeDevCommand` builds `gokod` and
starts Vite on port `5173`; you do not need to start the daemon separately. Vite
uses `strictPort`, so stop a stale Goko dev server if port `5173` is already in
use.

To keep development data separate from your normal Goko data, set `GOKO_HOME`:

```sh
GOKO_HOME=/tmp/goko-dev pnpm -C apps/desktop tauri dev
```

Without that variable, macOS data is stored under
`~/Library/Application Support/Goko`.

## Run the native Rune shell

The experimental native frontend (`apps/rune`, Cargo package `goko-rune`)
builds and runs through the workspace — no pnpm or Vite needed:

```sh
cargo run -p goko-rune
```

It shares `goko.db` and `crates/app-core` with the desktop app but never
connects to `gokod`: the terminal and session chrome is mock data, while the
Editor view is a real workspace over registered projects and checkouts
(Files, Changes, History, Stashes, Servers). See
[ARCHITECTURE.md](ARCHITECTURE.md#shared-app-core-and-the-native-rune-shell).

## Build macOS bundles

Build for the current Mac architecture and generate the `.app` and `.dmg`:

```sh
pnpm -C apps/desktop tauri build --bundles app,dmg
```

To package just one format, use `--bundles app` or `--bundles dmg`. Build
artifacts go under `target/release/bundle/`; this workspace's Cargo target
directory is at the repository root.

On an Apple Silicon Mac the native target is `aarch64-apple-darwin`; Intel Macs
use `x86_64-apple-darwin`. To build a universal macOS app, install both Rust
targets and pass Tauri's universal target:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
pnpm -C apps/desktop tauri build --target universal-apple-darwin --bundles app,dmg
```

See Tauri's [macOS bundle](https://v2.tauri.app/distribute/macos-application-bundle/)
and [DMG](https://v2.tauri.app/distribute/dmg/) guides for distribution details.
Building a bundle does not set up Developer ID signing or notarization; see
[Tauri's macOS signing guide](https://v2.tauri.app/distribute/sign/macos/) before
distributing it outside your development machine.

The build hook compiles and stages the matching `gokod` sidecar, so the app can
start its daemon from inside the `.app` bundle. Bundle artifacts are unsigned
unless a local signing identity is configured; they are intended for local
testing until Developer ID signing and notarization are set up.

The current build also warns that the bundle identifier `dev.goko.app` ends in
`.app`; Tauri recommends changing it before distribution.

## Other development checks

The root `dev` and `build` scripts run Vite only. Use the Tauri command above to
open the native desktop app.

```sh
pnpm typecheck
pnpm test
cargo test --workspace
cargo fmt --check
cargo clippy --workspace -- -D warnings
```

## Architecture

- [Architecture overview](ARCHITECTURE.md)
- [S1 system contract](docs/agent-devkit/s1-contract.md)

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Apache-2.0. See [LICENSE](LICENSE).

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versions follow the app release version (`[workspace.package]` in `Cargo.toml`).
Rename the `Unreleased` heading to the version and date when tagging a release.

## [Unreleased]

### Added

- `gokod` daemon crate: owns terminal PTY processes and serves `daemon.sock`
  under `GOKO_HOME` alongside `daemon.lock`, `daemon.token`, and `daemon.log`;
  single-instance enforcement and clean SIGTERM/SIGINT shutdown.
- `protocol` crate: shared wire messages between the app and `gokod`.
- `app-core` crate: `goko.db` store (projects, agents, sessions), Git and
  worktree operations, agent/account discovery, ACP, and language-server
  plumbing shared by the app.
- `goko-rune` native app (`apps/rune`): a GPU UI built on the Rune framework
  (wgpu/winit) — no webview and no Node toolchain.
- Sidebar driven by live daemon state: project → worktrees → sessions with
  hover row actions, a new-session menu, sub-agent indicators, stop/close
  controls, and a persisted drag-resize width.
- Terminal panes on an alacritty grid fed by `terminal.subscribe` streams:
  scrollback on the wheel, IME marked text (Telex) drawn at the cursor,
  drag-dropped files and images typed into the session, resizable splits,
  and panes kept inside the window.
- Code editor on Rune's `TextEdit`: file tree, find and content search, git
  Changes (stage/unstage/discard/commit/push), History with graph lanes,
  commit detail and unified/split patch views, stashes, Markdown preview,
  quick-open, go-to-definition, hover cards and definition lookups via
  language servers, and keyboard section navigation.
- Full-file syntax highlighting for ~80 languages through `rune-highlights`.
- Indentation aids: guide lines, Tab/Shift+Tab line and block indent, Enter
  auto-indent, and a persisted space/tab unit choice.
- Inline blame column: per-line author and commit age beside the code,
  caret-line annotations, hidden while the draft is dirty, toggled from the
  editor footer (`goko.editor.blame`).
- Settings page: agent accounts with sign-in, ACP agents, usage quota chips,
  theme, and persisted UI preferences.
- Native folder dialogs for project, worktree, and account pickers, with
  failures surfaced as notice toasts.
- Terminal output backpressure (`gokod` 0.1.2): `terminal.subscribe` accepts
  `flow: true`, holding unacknowledged output to a 256 KiB window — replay
  included — and dropping a stalled subscriber after 5 s; the app
  acknowledges parsed output with `terminal.ack` every 64 KiB and waits for
  the next ack before resubscribing a stalled stream.
- Claude Code and Codex usage indicators: header chips show known account
  limits and session rows show context fill.
- Terminal state in the daemon (`gokod` 0.1.3): each session keeps its own
  terminal model, and `terminal.subscribe` accepts `paint: true` to start the
  stream from a rebuild of the scrollback, screen, cursor and modes instead of
  the last 1 MiB of raw output, so reattaching shows the real screen. The app
  uses it on a daemon that supports it and answers the program's cursor and
  device-attribute queries on that stream.
- Settings → Updates: shows the running version, checks the latest GitHub
  release at startup, every six hours and on demand, tells you when a newer one
  with a build for your machine is out, and offers the install command to copy (`GOKO_NO_UPDATE_CHECK=1` turns
  the automatic check off).
- Release packaging for macOS (Apple Silicon): pushing a tag `vX.Y.Z` builds an
  ad hoc signed `Goko.app` in CI (no Apple Developer account), publishes it with
  a `latest.json` manifest and `install.sh` as a GitHub release, and
  `curl -fsSL …/install.sh | sh` installs or updates it after checking its
  sha256 and signature (`apps/rune/package-macos.sh`, `bundle-macos.sh`). The
  installer and `package-linux.sh` also handle Linux (x86_64, aarch64), which the
  workflow does not build yet.

### Changed

- The app shell is the native Rune client (`goko-rune`); the earlier
  Tauri + React + CodeMirror/xterm.js shell is gone.

### Fixed

- The terminal wheel follows the program's mode: mouse-reporting programs get
  wheel reports (SGR or legacy, Shift scrolls history instead), programs on the
  alternate screen get Up/Down keys, and everything else scrolls the history.
  It used to do nothing in a full-screen program.

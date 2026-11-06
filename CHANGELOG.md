# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versions follow the app release version (`[workspace.package]` in `Cargo.toml`,
mirrored in `apps/desktop/package.json` and `tauri.conf.json`). Rename the
`Unreleased` heading to the version and date when tagging a release.

## [Unreleased]

### Added

- `gokod` daemon crate: owns terminal PTY processes and serves `daemon.sock`
  under `GOKO_HOME` alongside `daemon.lock`, `daemon.token`, and `daemon.log`;
  single-instance enforcement and clean SIGTERM/SIGINT shutdown.
- `protocol` crate: shared wire messages between the desktop app and `gokod`.
- Tauri 2 + React + TypeScript (strict) desktop app under `apps/desktop`, with
  Tailwind v4 semantic colour tokens.
- Projects, agents, and sessions backed by a SQLite store.
- Terminal UI (xterm.js) with PTY streaming over daemon IPC and session
  controls.
- Terminal output backpressure (`gokod` 0.1.2): `terminal.subscribe` accepts
  `flow: true`, holding unacknowledged output to a 256 KiB window — replay
  included — and dropping a stalled subscriber after 5 s; the WebView
  acknowledges parsed output with `terminal.ack` every 64 KiB and the bridge
  waits for the next ack before resubscribing a stalled stream.
- Claude Code and Codex usage indicators: header chips show known account
  limits, Claude session rows show context fill, and Goko captures Claude's
  statusline data while preserving the user's configured statusline command.
- CodeMirror editor in Files with line numbers, lazy syntax support, in-editor
  find, optional persisted Vim mode, and `:w` saving through the existing file
  save path.

### Changed

- Sidebar declutter: project rows show only the branch (full path moves to the
  tooltip), changed/unknown git status is a dot, row actions appear on hover or
  focus, session counts show only while collapsed, root-checkout sessions sit
  directly under their project, agent name leads each session row, and the
  header daemon pill shows only when not connected.
- Files adds Name and Content search modes; content results come from bounded,
  debounced Git search and open at their matching line through the unsaved-edit
  guard.

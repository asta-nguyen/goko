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

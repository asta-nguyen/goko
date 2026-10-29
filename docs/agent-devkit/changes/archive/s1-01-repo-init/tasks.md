# s1-01 Repo init and workspaces tasks

## Design

[s1-01 Repo init and workspaces](design.md)

## Global Constraints

- `Cargo.lock` is generated and left unignored for a future repository-owner commit; `target/`, `node_modules/`, `dist/`, `apps/desktop/src-tauri/gen/`, `.DS_Store` are ignored.
- No dependency is added beyond what the skeleton needs (none for these crates yet).
- No commit is made.

## Impact map

```text
Entry: no existing source; planned entry: crates/daemon/src/main.rs::main (gokod --version)
Flow: no existing flow; planned flow: cargo build → gokod binary prints version
State changes: none found
External effects: `.git` already exists, so `git init` is a no-op; pnpm lockfile created on install
Change candidates: all new — .gitignore, package.json, pnpm-workspace.yaml, Cargo.toml, crates/protocol/**, crates/daemon/**
Verification: cargo build/fmt/clippy; gokod --version; git check-ignore
Verified at: no commit exists
```

## Tasks

### Task 1 — Git repo and pnpm workspace

Files: .gitignore, package.json, pnpm-workspace.yaml
Interfaces: produces the repo root every later plan builds in.
Change: `git init` with no commit. `.gitignore` covers `target/`, `node_modules/`, `dist/`, `apps/desktop/src-tauri/gen/`, `.DS_Store`. Root `package.json` is `"private": true` with scripts `dev`, `build`, `typecheck`, `test` that run the same script in `apps/desktop` via `pnpm -C apps/desktop`. `pnpm-workspace.yaml` lists `apps/*`.
Verify: `git rev-parse --is-inside-work-tree` prints `true`; individual `git check-ignore -q` checks succeed for `target/`, `node_modules/`, `dist/`, `apps/desktop/src-tauri/gen/`, and `.DS_Store`, while `git check-ignore -q Cargo.lock` returns 1; `pnpm install` exits 0 with the workspace empty.
Progress: done
Result: `git rev-parse --is-inside-work-tree` → `true`; individual `git check-ignore -q` checks for `target/`, `node_modules/`, `dist/`, `apps/desktop/src-tauri/gen/`, and `.DS_Store` → exit 0; the `Cargo.lock` check → exit 1; `pnpm install` → exit 0.

### Task 2 — Cargo workspace with `protocol` and `daemon` skeletons

Files: Cargo.toml, crates/protocol/Cargo.toml, crates/protocol/src/lib.rs::PROTOCOL_VERSION, crates/daemon/Cargo.toml, crates/daemon/src/lib.rs, crates/daemon/src/main.rs::main
Interfaces: produces crate `protocol` (const `PROTOCOL_VERSION = 1`) and crate `daemon` (empty lib plus bin `gokod`), consumed by all later Rust plans.
Change: workspace members are `crates/protocol` and `crates/daemon` only (the Tauri crate joins in s1-02). `gokod --version` prints `gokod <CARGO_PKG_VERSION>` and exits 0; any other args print `usage: gokod --version` to stderr and exit 2. Edition 2021, `[workspace.package]` carries the shared version.
Verify: `cargo build --workspace` passes with no warnings; `cargo run -q -p daemon --bin gokod -- --version` prints `gokod 0.1.0`; running with no args, with `--bad`, and with `--version --bad` each exits 2 and prints exactly `usage: gokod --version` to stderr; `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` pass.
Progress: done
Result: `cargo build --workspace` → exit 0 with no warnings; `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` → exit 0; `--version` → stdout `gokod 0.1.0`, exit 0; no args, `--bad`, and `--version --bad` → stderr `usage: gokod --version`, exit 2.

## Files inspected, no change

- None — the project has no application source yet; `.claude/skills` and `skills-lock.json` are tooling only.

## Approval Gate

Status: approved

## Decision Log

None.

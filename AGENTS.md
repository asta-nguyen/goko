# Agent instructions

## Purpose

Goko is planned as a native desktop app for running interactive commands in a terminal owned by a separate Rust daemon. The approved S1 contract is in [s1-contract.md](docs/agent-devkit/s1-contract.md).

## Project layout

The Rust protocol and daemon live in `crates/protocol/` and `crates/daemon/`, shared backend logic in `crates/app-core/`, and the native app in `apps/rune/`. The former Tauri/React app (`apps/desktop`) has been removed.

## Verification

- `cargo build --workspace`, `cargo fmt --check`, and `cargo clippy --workspace -- -D warnings` — passed.
- `cargo test --workspace` runs the protocol and daemon tests (the daemon integration tests start the real `gokod` in a temp `GOKO_HOME`).
- `gokod --version` prints `gokod 0.1.0`; an unsupported arg prints the usage and exits 2; no args runs the daemon: it holds `daemon.lock` in `GOKO_HOME` (default `~/Library/Application Support/Goko`), serves `daemon.sock`, writes `daemon.token` and `daemon.log`, and exits cleanly on SIGTERM/SIGINT. A second `gokod` prints `gokod: already running` and exits 0.
- The observed checks are recorded in [s1-01 task results](docs/agent-devkit/changes/archive/s1-01-repo-init/tasks.md).

The Rune app is a native binary (`apps/rune` → `goko-rune`). On macOS a bare binary opens **no window** — without a bundle, LaunchServices never activates the app, so its window is never ordered in. Run it through `apps/rune/run-macos.sh [release|debug]`, which builds the workspace, assembles `target/Goko.app` (the app plus `gokod` beside it, so the app can start the daemon) and launches it. The daemon client spawns `gokod` out of `GOKO_HOME` (`app_core::home::home_dir`), so a fresh home gets its own daemon.

A release is a pushed tag `vX.Y.Z`: `.github/workflows/release.yml` builds macOS (Apple Silicon) with `apps/rune/package-macos.sh` and Linux (x86_64 on `ubuntu-22.04`, aarch64 on `ubuntu-22.04-arm`) with `apps/rune/package-linux.sh` (each a tar.gz and a `<target>.target.json` entry), merges the entries into `latest.json` with `jq` (with `notes`, the `CHANGELOG.md` entry for the tag, which Settings → Updates shows), and publishes them with `distribution/install.sh` as a GitHub release; users install with the `curl` command in the README. `bundle-macos.sh` is the one place the macOS bundle is assembled, shared with `run-macos.sh`. There is no Windows release (`gokod` uses Unix sockets).

### Cutting a release

1. Bump `[workspace.package] version` in `Cargo.toml`; `cargo build` refreshes `Cargo.lock`. `gokod` has its own version in `crates/daemon/Cargo.toml`: bump it only when the app gates on a new daemon feature (`version_at_least` in `apps/rune/src/daemon.rs`).
2. In `CHANGELOG.md`, put the pending entries under `## [X.Y.Z] - YYYY-MM-DD` and leave an empty `## [Unreleased]` above it. That entry is what users read as "What's new" when the app offers the update, so write it for them.
3. `cargo fmt --all -- --check` and `cargo test --workspace` pass.
4. Merge to `main`, then tag that commit `vX.Y.Z` and push the tag. The push is what builds and publishes, so do it last and only on purpose.
5. When the `release` workflow finishes, check that the GitHub release has the tar.gz, `latest.json` and `install.sh`.

### Checking UI changes in the running app

Unit tests and the rune test harness do not draw a window, so a UI change is only verified once it is seen in the real app. Use `agent-desktop` (installed at `/opt/homebrew/bin/agent-desktop`; read `agent-desktop skills get desktop` first) to read the accessibility tree, click and screenshot **the Goko window only**.

- **Know which build is running.** `/Applications/Goko.app` is a separate, installed copy that `run-macos.sh` never updates; it goes stale. Confirm the binary under test is the fresh `target/Goko.app` (compare `ps -o command -p <pid>` and the binary's mtime with `git log -1`) before judging a fix.
- **Name the target.** `--app Goko` fails with `AMBIGUOUS_TARGET` when more than one instance matches (the installed app, its `gokod`). Launch a copy of `target/Goko.app` with a unique bundle id and name (`open -n`) and pass that name to `--app`.
- **Never take a full-screen screenshot** (`screencapture`): the display shows the user's unrelated apps. Use `agent-desktop screenshot --app <name>` or the accessibility tree.
- **Default to headless, read-only interaction** (snapshot, find, get, semantic click). Use `--headed` (physical input, takes the foreground) only when the user has allowed it, and do not click anything that writes or deletes real data.

## Skills

- `brainstorm-feature`: `.agents/skills/brainstorm-feature/SKILL.md`
- `context-handoff`: `.agents/skills/context-handoff/SKILL.md`
- `document-wiki`: `.agents/skills/document-wiki/SKILL.md`
- `estimate-feature`: `.agents/skills/estimate-feature/SKILL.md`
- `implement-task`: `.agents/skills/implement-task/SKILL.md`
- `lean-audit`: `.agents/skills/lean-audit/SKILL.md`
- `plan-feature`: `.agents/skills/plan-feature/SKILL.md`
- `read-codebase-context`: `.agents/skills/read-codebase-context/SKILL.md`
- `review-and-verify`: `.agents/skills/review-and-verify/SKILL.md`
- `setup-codebase`: `.agents/skills/setup-codebase/SKILL.md`
- `setup-openez`: `.agents/skills/setup-openez/SKILL.md`
- `systematic-debugging`: `.agents/skills/systematic-debugging/SKILL.md`
- `using-devkit`: `.agents/skills/using-devkit/SKILL.md`

## Code intelligence

Locate concepts with OpenEZ `code_query`, approximate filenames with FFF
fuzzy file find, identifiers or literals with FFF grep (fallback: `rg`),
and regex with `rg`. Expand callers and callees with OpenEZ `code_context`
(1–2 hops); confirm string, route, config-key, and case variants with FFF
multi-pattern grep (fallback: `rg`). Use OpenEZ `code_outline` before
reading a large file. Search results are navigation; read current source as
evidence. Query OpenEZ directly with the repository path; if unavailable,
unindexed, or irrelevant, continue with FFF or `rg` and direct reads.


## Delegation

Delegate leaf work only: 1–3 files, a result the reviewer can check, no change to a
public API, schema, dependency, CI config, or security boundary. Core logic,
cross-file refactors, and anything past the worker's context budget stay with the
orchestrator.

Codex subagent defaults live in this project's `.codex/config.toml`.
Use `agent_type: "leaf_worker"` for leaf work; its instructions live in
`.codex/agents/leaf-worker.toml` and it inherits those defaults. Never pass
`model` or `reasoning_effort` when spawning. The orchestrator uses the session's
selected model. Claude's worker config lives in `.claude/agents/worker.md`.

Every sub-task brief states, in this order:

- the files it may edit — one owner per file, and no two workers in the same tree
- acceptance criteria that are each a checkable observation or command; no
  percentages, no "reasonable", no "clean"
- the verify command from [Verification](#verification) that proves it:
  `cargo build --workspace`, `cargo fmt --check`,
  `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`

A worker returns the diff and the verify output. If the brief is ambiguous it returns
the question instead of guessing, because a guessed retry spends one of two.

Review reads the diff against the criteria: pass or fail, nothing between. A fail
returns at most three line-specific points. After two failed attempts, counted in the
change's `tasks.md`, the orchestrator finishes that sub-task itself.

Keep worker prompts under ~100K tokens by passing the specific files and line ranges,
never whole files. Put stable material first — this file, then conventions.

Final review of a change belongs to `review-and-verify`; this section does not
replace it.

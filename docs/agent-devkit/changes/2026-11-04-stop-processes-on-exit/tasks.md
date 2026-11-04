# Stop Goko-owned processes when the app exits tasks

## Design

[Stop Goko-owned processes when the app exits](design.md)

## Global Constraints

- Main-window close and Cmd+Q share one idempotent graceful shutdown path.
- Normal quit stops the daemon and all PTY child process groups, but preserves
  `GOKO_HOME` and app data.
- A daemon shutdown failure must not be presented as a successful clean exit.
- Finder deletion, Force Quit, crashes, and OS termination are outside the
  graceful cleanup guarantee.

## Impact map

Entry: `apps/desktop/src-tauri/src/lib.rs::run`

Flow: Tauri main-window close / Cmd+Q → shared shutdown coordinator → authenticated
`system.shutdown` request over `daemon.sock` → `SessionManager::shutdown()` terminates
PTY process groups → daemon replies, removes socket, and exits → desktop app exits

State changes: daemon in-memory sessions become exited; PTY children terminate;
`daemon.sock` is removed; `GOKO_HOME` files and SQLite metadata remain

External effects: authenticated local socket request; SIGTERM/SIGKILL to live
session process groups; daemon log records shutdown

Change candidates: `crates/protocol/src/message.rs`; `crates/daemon/src/server.rs`;
`crates/daemon/src/main.rs`; `apps/desktop/src-tauri/src/daemon_client.rs`;
`apps/desktop/src-tauri/src/bridge.rs`; `apps/desktop/src-tauri/src/lib.rs`;
daemon and desktop shutdown checks; `docs/llm/architecture/overview.md`;
`ARCHITECTURE.md`

Verification: protocol request serialization; daemon integration shutdown with a
live PTY child; macOS manual close-window and Cmd+Q flows; unavailable and
unsupported daemon paths; documentation consistency

Verified at: `62095c24c37a211662d8be10e98142ec8f65627f` (current source also
includes the inspected working-tree state)

## Baseline

- `cargo test --workspace` — 205 passed across 21 suites.
- `cargo fmt --check` — passed.
- `cargo clippy --workspace -- -D warnings` — passed with no issues.

## Tasks

### Task 1 — Add authenticated daemon shutdown

Files: `crates/protocol/src/message.rs::Request`; `crates/daemon/src/server.rs::Client::dispatch`; `crates/daemon/src/main.rs::run`; `crates/daemon/tests/e2e.rs::sessions_run_stop_and_die_with_the_daemon`

Interfaces: add `system.shutdown` to the authenticated request protocol; connect it to the existing daemon shutdown path; acknowledge acceptance, then let the desktop wait for the daemon lock to be released before exit.

Change: acknowledge the shutdown request, stop accepting new work, terminate all PTY process groups through `SessionManager::shutdown()`, remove `daemon.sock`, and exit the daemon. The desktop does not treat the acknowledgement as completion and waits for the daemon lock to be released. Repeated shutdown requests are harmless. Unknown/older-daemon behavior remains an explicit error.

Verify: run the protocol serialization and daemon e2e checks; a shutdown request with a live child must leave no child PID or socket and the daemon must exit successfully.

Progress: done
Result: `cargo test -p protocol` (17 passed), `cargo test -p daemon --test e2e` (1 passed), and `cargo fmt --check` passed; the remote shutdown request ends the daemon and PTY child.

### Task 2 — Route close and Cmd+Q through shutdown

Files: `apps/desktop/src-tauri/src/lib.rs::run`; `apps/desktop/src-tauri/src/bridge.rs::Bridge`; `apps/desktop/src-tauri/src/daemon_client.rs::DaemonClient`

Interfaces: expose one bridge shutdown operation to the Tauri lifecycle; both close-request and app-exit events call it and share one in-flight result.

Change: prevent the main window/app from exiting until the daemon confirms shutdown. If there is no daemon, exit normally. If the daemon is live but shutdown fails or times out, keep the app open and show an error so the user can retry. Preserve app data.

Verify: on macOS, start a live shell and child process, close the main window, and confirm the app, daemon, and child PIDs exit; repeat with Cmd+Q. Confirm `goko.db` remains and that an unavailable daemon does not block exit.

Progress: todo
Result: none

### Task 3 — Refresh current architecture docs

Files: `docs/llm/architecture/overview.md`; `ARCHITECTURE.md`

Interfaces: documentation only; retain archived S1 contract and acceptance records as historical evidence.

Change: document that normal close and Cmd+Q stop the daemon and PTY sessions, app data remains, and Force Quit/crash/Finder deletion do not promise cleanup.

Verify: read both updated pages against current source and confirm no current-behavior claim says terminal sessions survive normal app quit.

Progress: todo
Result: none

## Files inspected, no change

- `apps/desktop/src-tauri/src/daemon_client.rs` — daemon is launched with `setsid` and the child handle is dropped.
- `crates/daemon/src/session.rs` — existing `SessionManager::shutdown()` terminates process groups with a two-second grace period.
- `docs/agent-devkit/s1-contract.md` — historical contract explicitly records detached-daemon behavior.
- `docs/agent-devkit/changes/archive/s1-13-acceptance/tasks.md` — records Cmd+Q leaving the daemon and shell alive in the direct-binary app run.

## Approval Gate

Status: approved

## Decision Log

- `system.shutdown` replies after the daemon accepts the request so the socket
  writer can flush before the daemon begins closing connections. The desktop
  treats lock release, not the RPC reply, as shutdown completion.

# s1-08 Daemon IPC wiring tasks

## Design

[s1-08 Daemon IPC wiring](design.md)

## Global Constraints

- Each connection has one writer task; responses, events, and output frames never interleave mid-frame.
- A client disconnect never stops a session.
- Max frame 1 MiB; every request still requires prior authentication.
- Tests use a temp `GOKO_HOME` and harmless programs only.

## Impact map

```text
Entry: crates/daemon/src/server.rs::dispatch (from s1-04, not yet written) — pending approval
Flow: no existing flow; planned flow: client → framed request → dispatch → SessionManager / PtyProcess → response; output: PtyProcess::subscribe → writer task → kind-1 frames — pending approval
State changes: in-memory sessions and per-connection subscriptions
External effects: child processes; SIGTERM shutdown behavior — pending approval
Change candidates: crates/daemon/src/{server,main}.rs, crates/daemon/tests/{session_ipc,subscribe_ipc,terminal_io_ipc,e2e}.rs
Verification: cargo test --workspace; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — `session.*` requests and `events.subscribe`

Files: crates/daemon/src/server.rs::{serve, dispatch}
Interfaces: consumes `SessionManager` (s1-07) and the s1-04 server; produces `session.create/list/get/stop` and event pushes.
Change: `serve` takes `Arc<SessionManager>`; the `gokod` main constructs it. Route the four session requests; map `SessionError` to `invalid_argument`/`not_found`. `events.subscribe` forwards manager broadcasts as `session.updated`/`session.exited` through the connection's writer task; a lagged broadcast receiver triggers a fresh `session.updated` for every session.
Verify: `cargo test -p daemon --test session_ipc` — create/list/get/stop work over the socket; a subscribed client sees running then exited events; an unsubscribed client sees none; `stop` with an unknown id → `not_found`; an empty argv → `invalid_argument`; bad cwd returns a session in `failed`.
Progress: todo
Result: none

### Task 2 — `terminal.subscribe` and output frames

Files: crates/daemon/src/server.rs::{dispatch, stream_output}
Interfaces: consumes `PtyProcess::subscribe` (s1-06) and Task 1; produces kind-1 output frames and `terminal.lagged`.
Change: on `terminal.subscribe`, reply with `{replay_from, next_seq}`, then a per-subscription task sends the snapshot and live `Chunk`s as kind-1 frames. `Lagged` sends `terminal.lagged` and ends the task; `Closed` ends it quietly. `terminal.unsubscribe` and connection close abort the task.
Verify: `cargo test -p daemon --test subscribe_ipc` — frames carry contiguous `seq`; a second client subscribing mid-stream gets snapshot + live without gap or duplicate; no frames after unsubscribe; killing the client connection leaves the session `running`; a stalled client receives `terminal.lagged`; subscribing to an exited session yields the snapshot then nothing.
Progress: todo
Result: none

### Task 3 — `terminal.write` and `terminal.resize`

Files: crates/daemon/src/server.rs::dispatch
Interfaces: consumes `SessionManager::pty` and Task 2.
Change: decode base64 and write to the session's PTY; resize through the s1-05 call. Map `PtyError::InvalidArgument` to `invalid_argument`, invalid base64 to `malformed`, unknown id to `not_found`.
Verify: `cargo test -p daemon --test terminal_io_ipc` — write `hello\n` to a `cat` session and receive the echo on a subscription; resize to 90×25 then `stty size` echoes `25 90`; invalid base64 → `malformed`; write to an exited session → `invalid_argument`.
Progress: todo
Result: none

### Task 4 — Shutdown hook, daemon end-to-end test, Rust gate

Files: crates/daemon/src/main.rs::main, crates/daemon/tests/e2e.rs
Interfaces: consumes Tasks 1–3 and `SessionManager::shutdown`.
Change: on SIGTERM/SIGINT call `shutdown()` before removing the socket. The e2e test starts the real `gokod` in a temp home, creates a `cat` session and a concurrent `sh` session, writes and reads echo, stops one gracefully and force-kills the other, starts a third, then SIGTERMs the daemon.
Verify: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` all pass; after the daemon exits, no session process pid is alive and the socket file is gone.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on code from s1-04, s1-06, s1-07, which does not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.

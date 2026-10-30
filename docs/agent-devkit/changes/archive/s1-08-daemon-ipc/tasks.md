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
Entry: crates/daemon/src/server.rs (`serve`, and the per-connection `Client::dispatch`)
Flow: client → framed request → dispatch → SessionManager / PtyProcess → response; output: PtyProcess::subscribe → stream_output task → writer task → kind-1 frames
State changes: in-memory sessions and per-connection subscriptions
External effects: child processes; SIGTERM shutdown behavior
Change candidates: crates/daemon/src/{server,main}.rs, crates/daemon/Cargo.toml (base64, already in the lockfile), crates/daemon/tests/{common/mod,lifecycle,dispatch,handshake,session_ipc,subscribe_ipc,terminal_io_ipc,e2e}.rs
Verification: cargo test --workspace; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — `session.*` requests and `events.subscribe`

Files: crates/daemon/src/server.rs::{serve, dispatch}
Interfaces: consumes `SessionManager` (s1-07) and the s1-04 server; produces `session.create/list/get/stop` and event pushes.
Change: `serve` takes `Arc<SessionManager>`; the `gokod` main constructs it. Route the four session requests; map `SessionError` to `invalid_argument`/`not_found`. `events.subscribe` forwards manager broadcasts as `session.updated`/`session.exited` through the connection's writer task; a lagged broadcast receiver triggers a fresh `session.updated` for every session.
Verify: `cargo test -p daemon --test session_ipc` — create/list/get/stop work over the socket; a subscribed client sees running then exited events; an unsubscribed client sees none; `stop` with an unknown id → `not_found`; an empty argv → `invalid_argument`; bad cwd returns a session in `failed`.
Progress: done
Result: `cargo test -p daemon --test session_ipc` — 5 tests pass over the real socket: create/list/get/stop work and a stopped `cat` ends `exited` with no exit code; a subscribed client sees `starting`, `running`, `stopping`, `exited` as `session.updated` plus one `session.exited`, while a client that never subscribed receives nothing; stop or get of an unknown id is `not_found`; empty argv and zero columns are `invalid_argument` and create no session; a bad cwd returns a `failed` session with a message; sessions list in creation order. `serve` now takes `Arc<SessionManager>` and one writer task per connection. Result shapes (documented in the `server.rs` header for s1-09): `session.create/get/stop` → `SessionInfo`, `session.list` → `{"sessions": [...]}`, everything else without data → `{}`. The path where a lagging event receiver is answered with a fresh `session.updated` per session is implemented but has no test.

### Task 2 — `terminal.subscribe` and output frames

Files: crates/daemon/src/server.rs::{dispatch, stream_output}
Interfaces: consumes `PtyProcess::subscribe` (s1-06) and Task 1; produces kind-1 output frames and `terminal.lagged`.
Change: on `terminal.subscribe`, reply with `{replay_from, next_seq}`, then a per-subscription task sends the snapshot and live `Chunk`s as kind-1 frames. `Lagged` sends `terminal.lagged` and ends the task; `Closed` ends it quietly. `terminal.unsubscribe` and connection close abort the task.
Verify: `cargo test -p daemon --test subscribe_ipc` — frames carry contiguous `seq`; a second client subscribing mid-stream gets snapshot + live without gap or duplicate; no frames after unsubscribe; killing the client connection leaves the session `running`; a stalled client receives `terminal.lagged`; subscribing to an exited session yields the snapshot then nothing.
Progress: done
Result: `cargo test -p daemon --test subscribe_ipc` — 7 tests pass: output frames start at `replay_from`, have contiguous `seq`, and equal the retained buffer; a second client subscribing mid-stream gets snapshot + live equal to the buffer, with no gap or duplicate; no output frame follows an unsubscribe (the task is awaited before the response); dropping a connection leaves the session `running` and the subscription is removed on the next output; a client that subscribes and never reads receives `terminal.lagged` after its queued frames while a second client still gets every byte and the session exits 0; subscribing to an exited session yields the snapshot, then nothing; the response always precedes the first output frame (a mutation that starts streaming first fails this test 3 of 3).

### Task 3 — `terminal.write` and `terminal.resize`

Files: crates/daemon/src/server.rs::dispatch
Interfaces: consumes `SessionManager::pty` and Task 2.
Change: decode base64 and write to the session's PTY; resize through the s1-05 call. Map `PtyError::InvalidArgument` to `invalid_argument`, invalid base64 to `malformed`, unknown id to `not_found`.
Verify: `cargo test -p daemon --test terminal_io_ipc` — write `hello\n` to a `cat` session and receive the echo on a subscription; resize to 90×25 then `stty size` echoes `25 90`; invalid base64 → `malformed`; write to an exited session → `invalid_argument`.
Progress: done
Result: `cargo test -p daemon --test terminal_io_ipc` — 4 tests pass: `hello\n` written to `cat` comes back on the subscription; after a resize to 90×25, `stty size` prints `25 90`; invalid base64 is `malformed`; resize 0 or above 1000 is `invalid_argument`; unknown ids are `not_found`; subscribing to a failed session is `invalid_argument`; write or resize on an exited session is `invalid_argument` with a message. Writes run in `spawn_blocking` because a child that does not read its input can block the PTY write.

### Task 4 — Shutdown hook, daemon end-to-end test, Rust gate

Files: crates/daemon/src/main.rs::main, crates/daemon/tests/e2e.rs
Interfaces: consumes Tasks 1–3 and `SessionManager::shutdown`.
Change: on SIGTERM/SIGINT call `shutdown()` before removing the socket. The e2e test starts the real `gokod` in a temp home, creates a `cat` session and a concurrent `sh` session, writes and reads echo, stops one gracefully and force-kills the other, starts a third, then SIGTERMs the daemon.
Verify: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` all pass; after the daemon exits, no session process pid is alive and the socket file is gone.
Progress: done
Result: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` pass (114 tests: protocol 17, daemon 78 across unit and integration tests, desktop 19). `cargo test -p daemon --test e2e` runs the real `gokod` in a temp home: `cat` and a shell run concurrently with echo round-trip, one is stopped gracefully and the other force-killed, a third session that ignores SIGHUP is still running at SIGTERM, then the daemon exits 0 within 5 s, the socket file is gone, and no session pid is alive. Removing `sessions.shutdown()` from `main` makes this test fail with `a session process outlived the daemon`; the session ignores SIGHUP because closing the PTY would otherwise kill it without the daemon's help. `gokod` now calls `SessionManager::shutdown()` before removing the socket. The suite passed 3 consecutive runs. The `s1-04` dispatch test that used `session.list` as its unimplemented example now checks an unknown session id instead.

## Files inspected, no change

- `crates/daemon/src/{session,pty}.rs` (s1-05 to s1-07, archived) — `SessionManager` and `PtyProcess::subscribe` consumed as-is.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.

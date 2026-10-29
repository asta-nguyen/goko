# s1-04 Daemon server and lifecycle tasks

## Design

[s1-04 Daemon server and lifecycle](design.md)

## Global Constraints

- Daemon dir 0700; `daemon.sock` and `daemon.token` 0600.
- Every connection authenticates with `Hello` before any request is served.
- Max frame 1 MiB; a bad frame closes only that connection.
- Tests use a temp `GOKO_HOME`, never the real support directory.

## Impact map

```text
Entry: no existing source; planned entry: crates/daemon/src/main.rs::main and crates/daemon/src/server.rs::serve — pending approval
Flow: no existing flow; planned flow: gokod main → InstanceLock → create_token → server::bind/serve → handshake → dispatch — pending approval
State changes: files in GOKO_HOME (daemon.lock, daemon.sock, daemon.token, daemon.log) — pending approval
External effects: Unix socket listener; SIGTERM/SIGINT handling — pending approval
Change candidates: crates/daemon/src/{paths,lock,auth,server,main,lib}.rs, crates/daemon/Cargo.toml, crates/daemon/tests/{handshake,dispatch,lifecycle}.rs
Verification: cargo test -p daemon; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — Paths and single-instance lock

Files: crates/daemon/src/paths.rs::Paths, crates/daemon/src/lock.rs::InstanceLock
Interfaces: produces `Paths::resolve()` and `InstanceLock::acquire(&Paths)` used by every later daemon task.
Change: resolve the support directory (env `GOKO_HOME` first), create it with mode 0700, expose the four file paths. `acquire` takes a non-blocking exclusive `flock` on `daemon.lock` and returns `AlreadyRunning` if held; the lock is released on drop.
Verify: `cargo test -p daemon paths lock` — directory mode is 0700; a second acquire in the same home fails; after dropping the first it succeeds; `GOKO_HOME` is honoured.
Progress: todo
Result: none

### Task 2 — Auth token

Files: crates/daemon/src/auth.rs::{create_token, Token::verify}
Interfaces: consumes `Paths`; produces the token the server checks and clients read.
Change: read 32 bytes from `/dev/urandom`, write 64 hex chars to `daemon.token` with mode 0600 (no new dependency). `verify` compares in constant time and rejects any length mismatch.
Verify: `cargo test -p daemon auth` — file mode is 0600 and length 64; two tokens differ; `verify` accepts the right value and rejects a wrong or different-length one.
Progress: todo
Result: none

### Task 3 — Socket server: bind, accept, handshake

Files: crates/daemon/src/server.rs::{bind, serve}
Interfaces: consumes s1-03 frames and messages plus Tasks 1–2; produces a Tokio Unix-socket server that authenticates each connection, then calls `dispatch` (Task 4).
Change: remove a stale socket, bind with mode 0600, accept any number of clients. The first frame must be a valid `Hello`; wrong token → `auth_failed`, unsupported version → `version_unsupported`, sent as an error frame before closing. On success reply `HelloOk`.
Verify: `cargo test -p daemon --test handshake` with a test client — good hello gets `HelloOk`; wrong token and wrong version get the right code and the socket closes; a leftover socket file is replaced; socket mode is 0600.
Progress: todo
Result: none

### Task 4 — Dispatch, health/version, bad-frame isolation

Files: crates/daemon/src/server.rs::dispatch
Interfaces: consumes Task 3; produces the request router that s1-08 extends.
Change: answer `system.health` (uptime seconds) and `system.version`; any other request returns `not_found`. Malformed JSON returns `malformed`; an oversized or truncated frame closes only that connection.
Verify: `cargo test -p daemon --test dispatch` — health and version respond; an unimplemented request → `not_found`; client A sends an oversized frame and is closed while client B keeps getting health responses.
Progress: todo
Result: none

### Task 5 — `gokod` main: lock, serve, graceful shutdown

Files: crates/daemon/src/main.rs::main
Interfaces: consumes Tasks 1–4; produces the runnable daemon (replaces the `--version`-only main from s1-01, keeping `--version`).
Change: acquire the lock (if held print `gokod: already running`, exit 0), create the token, init `tracing` to `daemon.log`, serve. On SIGTERM or SIGINT stop accepting, remove `daemon.sock`, exit 0.
Verify: `cargo test -p daemon --test lifecycle` runs the real `gokod` in a temp home — a client gets health; a second `gokod` exits 0 while the first keeps answering; SIGTERM removes the socket file and the process exits within 2 s; `gokod --version` still works.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on the crates from s1-01 and s1-03, which do not exist yet.

## Approval Gate

Status: approved

## Decision Log

None.

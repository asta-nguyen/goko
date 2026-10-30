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
Entry: crates/daemon/src/main.rs::main (version-only before this change); new: crates/daemon/src/server.rs::serve
Flow: gokod main → InstanceLock → create_token → server::bind/serve → handshake → dispatch
State changes: files in GOKO_HOME (daemon.lock, daemon.sock, daemon.token, daemon.log)
External effects: Unix socket listener; SIGTERM/SIGINT handling
Change candidates: crates/daemon/src/{paths,lock,auth,server,main,lib,test_support}.rs, crates/daemon/Cargo.toml (adds protocol, serde, serde_json, tracing, tracing-subscriber and tokio features), crates/daemon/tests/{common/mod,handshake,dispatch,lifecycle}.rs, Cargo.lock
Verification: cargo test -p daemon; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — Paths and single-instance lock

Files: crates/daemon/src/paths.rs::Paths, crates/daemon/src/lock.rs::InstanceLock
Interfaces: produces `Paths::resolve()` and `InstanceLock::acquire(&Paths)` used by every later daemon task.
Change: resolve the support directory (env `GOKO_HOME` first), create it with mode 0700, expose the four file paths. `acquire` takes a non-blocking exclusive `flock` on `daemon.lock` and returns `AlreadyRunning` if held; the lock is released on drop.
Verify: `cargo test -p daemon paths lock` — directory mode is 0700; a second acquire in the same home fails; after dropping the first it succeeds; `GOKO_HOME` is honoured.
Progress: done
Result: `cargo test -p daemon --lib -- paths lock` — 5 tests pass: the directory is created 0700 (nested parents included); a second acquire fails with `AlreadyRunning` and succeeds after the first is dropped; `GOKO_HOME` wins, empty `GOKO_HOME` falls back to `~/Library/Application Support/Goko`, and no `HOME` is an error. The lock uses std `File::try_lock` (flock), so `libc` is not needed for it.

### Task 2 — Auth token

Files: crates/daemon/src/auth.rs::{create_token, Token::verify}
Interfaces: consumes `Paths`; produces the token the server checks and clients read.
Change: read 32 bytes from `/dev/urandom`, write 64 hex chars to `daemon.token` with mode 0600 (no new dependency). `verify` compares in constant time and rejects any length mismatch.
Verify: `cargo test -p daemon auth` — file mode is 0600 and length 64; two tokens differ; `verify` accepts the right value and rejects a wrong or different-length one.
Progress: done
Result: `cargo test -p daemon --lib -- auth` — 3 tests pass: `daemon.token` is 0600 with 64 hex characters; two tokens differ and the file holds the latest; `verify` accepts the right value and rejects a wrong one, a 63- and a 65-character one, and an empty one.

### Task 3 — Socket server: bind, accept, handshake

Files: crates/daemon/src/server.rs::{bind, serve}
Interfaces: consumes s1-03 frames and messages plus Tasks 1–2; produces a Tokio Unix-socket server that authenticates each connection, then calls `dispatch` (Task 4).
Change: remove a stale socket, bind with mode 0600, accept any number of clients. The first frame must be a valid `Hello`; wrong token → `auth_failed`, unsupported version → `version_unsupported`, sent as an error frame before closing. On success reply `HelloOk`.
Verify: `cargo test -p daemon --test handshake` with a test client — good hello gets `HelloOk`; wrong token and wrong version get the right code and the socket closes; a leftover socket file is replaced; socket mode is 0600.
Progress: done
Result: `cargo test -p daemon --test handshake` — 7 tests pass: a good hello gets `HelloOk`; a wrong token gets `auth_failed` and a wrong version `version_unsupported`, each followed by close; a request sent before the hello is answered `malformed` and the connection closes with nothing served; a client leaving mid-handshake does not disturb the server; a leftover socket file is replaced; the socket is 0600.

### Task 4 — Dispatch, health/version, bad-frame isolation

Files: crates/daemon/src/server.rs::dispatch
Interfaces: consumes Task 3; produces the request router that s1-08 extends.
Change: answer `system.health` (uptime seconds) and `system.version`; any other request returns `not_found`. Malformed JSON returns `malformed`; an oversized or truncated frame closes only that connection.
Verify: `cargo test -p daemon --test dispatch` — health and version respond; an unimplemented request → `not_found`; client A sends an oversized frame and is closed while client B keeps getting health responses.
Progress: done
Result: `cargo test -p daemon --test dispatch` — 6 tests pass: health returns `ok` with uptime and version returns both versions, echoing the numeric id; `session.list` is `not_found`; invalid JSON, an unknown `type`, and a request without `id` each return `malformed` and the connection keeps working; an oversized frame returns `frame_too_large` and closes only that connection while another client keeps getting health; a truncated frame closes only that connection. Connection-level errors use `Response` with `id` 0, so clients number requests from 1 (documented in `server.rs`).

### Task 5 — `gokod` main: lock, serve, graceful shutdown

Files: crates/daemon/src/main.rs::main
Interfaces: consumes Tasks 1–4; produces the runnable daemon (replaces the `--version`-only main from s1-01, keeping `--version`).
Change: acquire the lock (if held print `gokod: already running`, exit 0), create the token, init `tracing` to `daemon.log`, serve. On SIGTERM or SIGINT stop accepting, remove `daemon.sock`, exit 0.
Verify: `cargo test -p daemon --test lifecycle` runs the real `gokod` in a temp home — a client gets health; a second `gokod` exits 0 while the first keeps answering; SIGTERM removes the socket file and the process exits within 2 s; `gokod --version` still works.
Progress: done
Result: `cargo test -p daemon --test lifecycle` — 4 tests pass with the real `gokod` in a temp `GOKO_HOME`: health works; a second `gokod` prints `gokod: already running`, exits 0, and the first keeps answering; SIGTERM exits 0 within 2 s and removes the socket file; a SIGKILLed daemon's stale socket is recovered by the next start; `gokod --version` still prints `gokod 0.1.0` and an unknown flag exits 2. A manual run showed 0700 directory, 0600 files, and log lines for start and stop. The full daemon suite (44 tests) passed 5 consecutive runs; `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` are clean.

## Files inspected, no change

- `crates/protocol/src/{frame,message,session}.rs` (s1-03, archived) — frame codec and message types consumed as-is.
- `crates/daemon/src/pty.rs`, `buffer.rs` (s1-05, archived) — unrelated to the socket server; untouched.

## Approval Gate

Status: approved

## Decision Log

None.

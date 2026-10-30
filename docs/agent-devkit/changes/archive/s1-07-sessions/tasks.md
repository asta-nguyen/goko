# s1-07 Session manager tasks

## Design

[s1-07 Session manager](design.md)

## Global Constraints

- Session state only changes through `transition`; invalid edges are rejected.
- `stop` is idempotent and never signals an exited process.
- Spawn failure is a `failed` session, not an RPC error.
- Stop sends SIGTERM to the process group; force sends SIGKILL.

## Impact map

```text
Entry: crates/daemon/src/session.rs::SessionManager (new)
Flow: SessionManager::create → PtyProcess::spawn → exit watcher → transition → broadcast; stop → terminate/kill
State changes: in-memory session table
External effects: child processes; signals to process groups
Change candidates: crates/daemon/src/{session,lib,pty}.rs (`validate_size` becomes crate-visible), crates/daemon/tests/session_manager.rs
Verification: cargo test -p daemon session; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — State transitions

Files: crates/daemon/src/session.rs::{transition, Transition, InvalidTransition}
Interfaces: consumes `SessionState` from s1-03; produces a pure transition function used by Tasks 2–3.
Change: encode the allowed edges from the design as events (`Spawning`, `Spawned`, `StopRequested`, `ProcessExited`, `SpawnFailed`); every other (state, event) pair returns `InvalidTransition`. No I/O.
Verify: `cargo test -p daemon session::transition` — each allowed edge succeeds; every other pair among created/starting/running/stopping/exited/failed is rejected; exited and failed accept no event.
Progress: done
Result: `cargo test -p daemon --lib -- session` — 2 tests pass: over all 11 states × 5 events exactly the seven allowed edges succeed (created→starting, starting→running, running→stopping, running|stopping→exited, created|starting→failed) and everything else is `InvalidTransition`; exited and failed accept no event.

### Task 2 — SessionManager: create, list, get, stop, events

Files: crates/daemon/src/session.rs::{SessionManager, SessionError}
Interfaces: consumes Task 1, `PtyProcess` (s1-05), and `StreamItem` (s1-06); produces `create`, `list`, `get`, `stop(id, force)`, `pty(id)`, and `subscribe_events()`.
Change: implement the behavior in the design. `create` validates, spawns, and drives created→starting→running (or →failed). A watcher task awaits `wait_exit` and applies `ProcessExited`. `stop` takes the session lock, applies `StopRequested` once, then signals. Each state change broadcasts the full `SessionInfo`.
Verify: `cargo test -p daemon --test session_manager` — a `cat` session reaches running; stop twice is idempotent (one signal); stop after exit sends none; unknown id → `NotFound`; bad cwd ends `failed` with a message; two sessions have independent output; events arrive in order running → stopping → exited.
Progress: done
Result: `cargo test -p daemon --test session_manager` — 8 tests pass for Task 2: a `cat` session reaches running and its events arrive starting → running → stopping → exited; the exit code is recorded (`exit 7` → `Some(7)`), a process killed by a signal has `exit_code: None`, title defaults to the `argv[0]` basename or the trimmed given title, and `list` keeps creation order; two simultaneous stops plus a later one produce exactly one SIGTERM (a shell trap counts `got-term`; a mutation that lets a repeat stop re-signal fails this test with 2 ≠ 1), and a forced stop escalates a pending stop to SIGKILL; stop after exit returns the session, sends nothing, and emits no event; an unknown id is `NotFound`; a bad cwd and a missing executable become listed `failed` sessions with a message; empty argv, zero columns, and 1001 rows are `InvalidArgument` and create no session; two sessions keep independent output. Rulings: `stop` on a created/starting session is `InvalidArgument` (it cannot be reached through `create`, which returns a running session); `pty(id)` on a failed session is `InvalidArgument`.

### Task 3 — Manager shutdown

Files: crates/daemon/src/session.rs::SessionManager::shutdown
Interfaces: consumes Task 2; hooked into the daemon main in s1-08.
Change: `terminate` every live session, wait up to 2 s for exits, then `kill` the remainder; returns when all are exited.
Verify: `cargo test -p daemon --test session_manager shutdown` — three `sleep 30` sessions plus one that ignores SIGTERM are all exited within 3 s; no processes remain (checked with `kill -0` on the pids); calling it with no sessions returns immediately.
Progress: done
Result: `cargo test -p daemon --test session_manager shutdown` — 2 tests pass: three `sleep 30` sessions plus one that traps and ignores SIGTERM are all exited in under 3 s, and `kill -0` finds none of their pids; shutdown with no sessions returns in under 200 ms. The suite passed 5 consecutive runs; `cargo clippy -p daemon --all-targets -- -D warnings`, `cargo fmt -p daemon --check`, and `cargo test -p daemon` are clean.

## Files inspected, no change

- `crates/protocol/src/session.rs` (s1-03) — `SessionState`/`SessionInfo` used as-is.
- `crates/daemon/src/pty.rs` (s1-05, s1-06) — `PtyProcess::{spawn, terminate, kill, wait_exit, pid, buffer}` consumed; only `validate_size` visibility changed.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.

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
Entry: no existing source; planned entry: crates/daemon/src/session.rs::SessionManager — pending approval
Flow: no existing flow; planned flow: SessionManager::create → PtyProcess::spawn → exit watcher → transition → broadcast; stop → terminate/kill — pending approval
State changes: in-memory session table
External effects: child processes; signals to process groups — pending approval
Change candidates: crates/daemon/src/{session,lib}.rs, crates/daemon/tests/session_manager.rs
Verification: cargo test -p daemon session; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — State transitions

Files: crates/daemon/src/session.rs::{transition, Transition, InvalidTransition}
Interfaces: consumes `SessionState` from s1-03; produces a pure transition function used by Tasks 2–3.
Change: encode the allowed edges from the design as events (`Spawning`, `Spawned`, `StopRequested`, `ProcessExited`, `SpawnFailed`); every other (state, event) pair returns `InvalidTransition`. No I/O.
Verify: `cargo test -p daemon session::transition` — each allowed edge succeeds; every other pair among created/starting/running/stopping/exited/failed is rejected; exited and failed accept no event.
Progress: todo
Result: none

### Task 2 — SessionManager: create, list, get, stop, events

Files: crates/daemon/src/session.rs::{SessionManager, SessionError}
Interfaces: consumes Task 1, `PtyProcess` (s1-05), and `StreamItem` (s1-06); produces `create`, `list`, `get`, `stop(id, force)`, `pty(id)`, and `subscribe_events()`.
Change: implement the behavior in the design. `create` validates, spawns, and drives created→starting→running (or →failed). A watcher task awaits `wait_exit` and applies `ProcessExited`. `stop` takes the session lock, applies `StopRequested` once, then signals. Each state change broadcasts the full `SessionInfo`.
Verify: `cargo test -p daemon --test session_manager` — a `cat` session reaches running; stop twice is idempotent (one signal); stop after exit sends none; unknown id → `NotFound`; bad cwd ends `failed` with a message; two sessions have independent output; events arrive in order running → stopping → exited.
Progress: todo
Result: none

### Task 3 — Manager shutdown

Files: crates/daemon/src/session.rs::SessionManager::shutdown
Interfaces: consumes Task 2; hooked into the daemon main in s1-08.
Change: `terminate` every live session, wait up to 2 s for exits, then `kill` the remainder; returns when all are exited.
Verify: `cargo test -p daemon --test session_manager shutdown` — three `sleep 30` sessions plus one that ignores SIGTERM are all exited within 3 s; no processes remain (checked with `kill -0` on the pids); calling it with no sessions returns immediately.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on code from s1-03, s1-05, s1-06, which does not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.

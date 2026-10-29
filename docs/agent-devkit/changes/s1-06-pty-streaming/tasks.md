# s1-06 Gap-free output streaming tasks

## Design

[s1-06 Gap-free output streaming](design.md)

## Global Constraints

- Snapshot and subscriber registration happen under the same lock the reader appends under.
- The PTY reader never awaits a subscriber; per-subscriber queue is 256 chunks.
- `seq` is the cumulative byte offset of a chunk's first byte.

## Impact map

```text
Entry: no existing source; planned entry: crates/daemon/src/pty.rs::PtyProcess::subscribe — pending approval
Flow: no existing flow; planned flow: reader task → lock { buffer.append; for each subscriber try_send } ; subscribe → lock { snapshot; register } — pending approval
State changes: in-memory subscriber registry
External effects: none found
Change candidates: crates/daemon/src/pty.rs, crates/daemon/tests/{subscribe,lagged}.rs
Verification: cargo test -p daemon --test subscribe --test lagged
Verified at: no commit exists
```

## Tasks

### Task 1 — Gap-free subscribe

Files: crates/daemon/src/pty.rs::{PtyProcess::subscribe, StreamItem, Snapshot}
Interfaces: consumes `RingBuffer` and the reader from s1-05; produces `subscribe()` used by s1-08.
Change: put the buffer and the subscriber list behind one mutex. The reader appends and fans each chunk out (with its `seq`) under it. `subscribe` snapshots and registers under it. After the process ends the reader sends `Closed` to every subscriber; late subscribers get the snapshot then `Closed`.
Verify: `cargo test -p daemon --test subscribe` — while a process prints a numbered sequence, subscribing at a random moment yields snapshot + live bytes that form a contiguous, duplicate-free sequence (repeat 50 times); subscribing after exit yields snapshot then `Closed`; 20 subscribers receive identical streams.
Progress: todo
Result: none

### Task 2 — Slow-consumer drop

Files: crates/daemon/src/pty.rs::PtyProcess::subscribe
Interfaces: consumes Task 1.
Change: give each subscriber a bounded channel of 256 chunks. The reader uses `try_send`; on a full queue it removes that subscriber and delivers `Lagged` (best effort, via a slot that cannot itself block). A closed receiver is removed silently.
Verify: `cargo test -p daemon --test lagged` — a subscriber that never reads receives `Lagged` and is removed while a second subscriber receives every byte and the process still exits normally; dropping a receiver does not error or leak (registry length returns to 0).
Progress: todo
Result: none

## Files inspected, no change

- None — depends on `PtyProcess` and `RingBuffer` from s1-05, which do not exist yet.

## Approval Gate

Status: approved

## Decision Log

None.

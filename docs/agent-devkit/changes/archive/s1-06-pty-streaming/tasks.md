# s1-06 Gap-free output streaming tasks

## Design

[s1-06 Gap-free output streaming](design.md)

## Global Constraints

- Snapshot and subscriber registration happen under the same lock the reader appends under.
- The PTY reader never awaits a subscriber; per-subscriber queue is 256 chunks.
- `seq` is the cumulative byte offset of a chunk's first byte.

## Impact map

```text
Entry: crates/daemon/src/pty.rs::PtyProcess::subscribe (new); the reader thread in `spawn_reader` is the only producer
Flow: reader thread → lock { buffer.append; for each subscriber try_send } ; subscribe → lock { snapshot; register }
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
Progress: done
Result: `cargo test -p daemon --test subscribe` — 3 tests pass: subscribing at 50 pseudo-random moments while `sh` prints 5000 numbered lines yields snapshot + live chunks byte-for-byte equal to the daemon's retained buffer, each chunk's `seq` continuing the previous one, and the lines equal to the expected output (a run whose consumer lagged under CPU starvation is skipped, 50 complete runs are required, and at least one must have subscribed mid-stream); subscribing after exit yields the snapshot then `Closed`, and a late subscriber gets the snapshot and an immediate `Closed`; 20 subscribers on one PTY receive identical streams and an empty history has `replay_from == next_seq`. A mutation that snapshots and registers under separate locks makes the contiguity test fail (`chunk does not continue the stream`). Under CPU load macOS's tty sometimes emits `\r\r\n` for `\n` (106 of 700 loaded runs differed from the ideal output while every subscriber stream still equalled the retained buffer), so the tests compare against the buffer and treat a doubled `\r` as equal; they passed 3 idle and 5 heavily loaded runs. Receivers are plain `tokio::sync::mpsc::Receiver<StreamItem>`; `PtyProcess::subscriber_count` was added so the registry can be checked.

### Task 2 — Slow-consumer drop

Files: crates/daemon/src/pty.rs::PtyProcess::subscribe
Interfaces: consumes Task 1.
Change: give each subscriber a bounded channel of 256 chunks. The reader uses `try_send`; on a full queue it removes that subscriber and delivers `Lagged` (best effort, via a slot that cannot itself block). A closed receiver is removed silently.
Verify: `cargo test -p daemon --test lagged` — a subscriber that never reads receives `Lagged` and is removed while a second subscriber receives every byte and the process still exits normally; dropping a receiver does not error or leak (registry length returns to 0).
Progress: done
Result: `cargo test -p daemon --test lagged` — 2 tests pass: a subscriber that never reads holds exactly 256 chunks, then receives `Lagged` and nothing after it, while a second subscriber receives every byte (equal to the retained buffer) and `cat` still exits 0; dropping a receiver is removed on the next output without error and the registry length returns to 0. Each queue has 257 slots; the last is reserved for the final `Lagged` or `Closed`, so the reader's `try_send` never needs to wait. `cargo clippy -p daemon --all-targets -- -D warnings`, `cargo fmt -p daemon --check`, and `cargo test -p daemon` are clean.

## Files inspected, no change

- `crates/daemon/src/buffer.rs::RingBuffer` (s1-05, archived) — used as-is under the shared lock.

## Approval Gate

Status: approved

## Decision Log

None.

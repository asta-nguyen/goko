# s1-05 PTY core and replay buffer tasks

## Design

[s1-05 PTY core and replay buffer](design.md)

## Global Constraints

- Replay buffer capacity 1 MiB; `seq` is the cumulative byte offset.
- Commands are `argv` arrays; no shell string interpolation.
- Signals go to the process group, not just the child pid.
- Tests use only harmless programs: `sh`, `cat`, `sleep`, `stty`.

## Impact map

```text
Entry: no existing PTY source; planned entry: crates/daemon/src/pty.rs::PtyProcess::spawn
Flow: no existing flow; planned flow: caller → PtyProcess::spawn → portable-pty master/child → reader task → RingBuffer; write/resize/terminate/kill act on master and process group
State changes: in-memory buffer only
External effects: child processes in PTYs; signals to process groups
Change candidates: crates/daemon/src/{buffer,pty,lib}.rs, crates/daemon/Cargo.toml, Cargo.lock, crates/daemon/tests/{pty_spawn,pty_io,pty_signal}.rs
Verification: cargo test -p daemon; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — Replay ring buffer

Files: crates/daemon/src/buffer.rs::RingBuffer
Interfaces: produces `append(&[u8]) -> start_seq` and `snapshot() -> (replay_from, Vec<u8>, next_seq)` used by Tasks 2 and s1-06.
Change: bounded byte ring with a 1 MiB capacity constant that tracks the cumulative offset of its first retained byte; eviction advances `replay_from`; a single append larger than capacity keeps only its last 1 MiB.
Verify: `cargo test -p daemon buffer` — appending past capacity keeps only the newest bytes with the right `replay_from`; offsets stay contiguous across wraps; an empty buffer has `replay_from == next_seq`; a 3 MiB append keeps 1 MiB and `next_seq == 3 MiB`.
Progress: done
Result: `cargo test -p daemon buffer` — 4 tests pass: an empty buffer has `replay_from == next_seq`; appending past capacity keeps the newest bytes with the right `replay_from`; offsets stay contiguous across repeated evictions; a 3 MiB append keeps the last 1 MiB with `next_seq == 3 MiB`.

### Task 2 — Spawn, output reader, exit status

Files: crates/daemon/src/pty.rs::{SpawnSpec, PtyProcess::spawn, PtyProcess::wait_exit, ExitStatus, PtyError}
Interfaces: consumes `RingBuffer` (Task 1); produces a `PtyProcess` whose output lands in its buffer and whose exit status is awaitable.
Change: validate input (non-empty `argv`, existing directory `cwd`, size 1..=1000), spawn with `portable-pty` using the env from the design, start the blocking reader and the exit waiter. Spawn failures return `PtyError::SpawnFailed` with a message naming the cause.
Verify: `cargo test -p daemon --test pty_spawn` — `sh -c 'echo hi; exit 7'` leaves `hi` in the buffer and exits with code 7; nonexistent cwd, a file as cwd, nonexistent executable, and empty argv each return the expected error.
Progress: done
Result: `cargo test -p daemon --test pty_spawn` — 6 tests pass: `sh -c 'echo hi; exit 7'` leaves `hi` in the buffer and exits with code 7; nonexistent cwd, a file as cwd, a nonexistent executable, empty argv, and zero columns each return the expected `PtyError`.

### Task 3 — Write and resize

Files: crates/daemon/src/pty.rs::{PtyProcess::write, PtyProcess::resize}
Interfaces: consumes Task 2.
Change: write input bytes to the master and flush; resize the PTY, rejecting 0 and values above 1000 in either dimension with `InvalidArgument`; write after exit returns `Io`.
Verify: `cargo test -p daemon --test pty_io` — writing `hello\n` to `cat` echoes it into the buffer; resizing to 100×30 then running `stty size` prints `30 100`; 0×0 and 1001×10 are rejected; write after exit errors.
Progress: done
Result: `cargo test -p daemon --test pty_io` — 5 tests pass: `hello` written to `cat` appears in the buffer; resize to 100×30 then `stty size` prints `30 100`; 0×0 and 1001×10 are `InvalidArgument`; write after exit is `Io`; non-UTF-8 output is kept as bytes.

### Task 4 — Process-group signals

Files: crates/daemon/src/pty.rs::{PtyProcess::terminate, PtyProcess::kill}
Interfaces: consumes Task 2; produces the signal operations behind stop and force stop in s1-07.
Change: `terminate` sends SIGTERM and `kill` sends SIGKILL to the child's process group with `killpg`; if the exit status is already recorded, return ok without signalling.
Verify: `cargo test -p daemon --test pty_signal` — SIGTERM ends `sleep 30` with a signal status; `sh -c 'trap "" TERM; sleep 30'` survives `terminate` and dies on `kill`; a grandchild from `sh -c 'sleep 30 & wait'` is gone after `kill`; both calls on an exited process return ok.
Progress: done
Result: `cargo test -p daemon --test pty_signal` — 4 tests pass: SIGTERM ends `sleep 30` with a signal status; a child that traps TERM survives `terminate` and dies on `kill`; the grandchild from `sleep 30 & wait` is gone after `kill`; both calls on an exited process return ok. The full daemon suite passed 5 consecutive runs; `cargo clippy -p daemon --all-targets -- -D warnings` and `cargo fmt --check` are clean. Review simplified `pty.rs` (399 → 306 lines): the process-group id is the pid, since portable-pty starts the child with `setsid`; one `signal_group` replaces five cfg-gated helpers; `write` and `resize` no longer hold the exit-state lock during I/O.

## Files inspected, no change

- `crates/daemon/src/main.rs` — the version-only CLI has no current PTY callers; the PTY crate is new.
- `Cargo.toml` — the existing workspace already includes the daemon crate.

## Approval Gate

Status: approved

## Decision Log

None.

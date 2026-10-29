# s1-05 PTY core and replay buffer

## Goal

A daemon-side PTY abstraction: spawn an `argv` in a directory, keep a bounded record of
its output, accept input and resize, and signal the whole process group. No sockets, no
subscribers, no session state. Implements the PTY and buffer rules of the
[S1 contract](../../../s1-contract.md).

## Scope

In: `daemon::buffer::RingBuffer` and `daemon::pty::PtyProcess`. Out: live subscription
(s1-06), session lifecycle (s1-07), IPC (s1-08). Depends on: s1-01.

## Behavior

- **RingBuffer:** capacity 1 MiB (constant). `append(bytes)` returns the cumulative
  offset of the first appended byte. `snapshot()` returns
  `(replay_from, bytes, next_seq)`, where `replay_from` is the offset of the oldest
  retained byte; eviction advances it. Offsets never reset or skip.
- **spawn:** `SpawnSpec{argv, cwd, cols, rows}`. Uses `portable-pty` with
  `TERM=xterm-256color` and `COLORTERM=truecolor`; the child gets its own session and
  controlling terminal, so its pid is also its process-group id. A dedicated blocking
  task reads the master into the `RingBuffer`; another waits for exit and records
  `ExitStatus{code, signal}`. Commands are `argv` arrays, never a shell string.
- **write / resize:** input bytes go to the master. Resize accepts 1..=1000 in both
  dimensions.
- **terminate / kill:** SIGTERM / SIGKILL to the process group via `killpg`. Calling
  either on an exited process returns ok and sends nothing.
- **Errors:** local `PtyError` = `SpawnFailed(msg)`, `InvalidArgument(msg)`, `Io(msg)`;
  s1-07/s1-08 map them to protocol errors.

## Edge cases

- Empty `argv`, missing or non-directory `cwd`, nonexistent or non-executable
  `argv[0]` → `SpawnFailed` with a readable message, no panic.
- Resize with 0 or >1000 → `InvalidArgument`.
- Write after exit → `Io` error, no panic.
- Non-UTF-8 and very large output → stored as bytes; buffer evicts, reader never blocks.
- A grandchild process (`sh -c 'sleep 30 & wait'`) dies with the group on `kill`.
- Child that ignores SIGTERM survives `terminate` and dies on `kill`.

## Interfaces

`RingBuffer::{new, append, snapshot}`; `SpawnSpec`; `PtyProcess::{spawn, write, resize,
terminate, kill, pid, buffer, wait_exit}`; `ExitStatus`; `PtyError`.
Dependencies added: `portable-pty`, `libc`, and `tokio`.

## Verification

Unit tests for `RingBuffer`; integration tests `pty_spawn`, `pty_io`, `pty_signal` using
`sh`, `cat`, `sleep`, `stty`; clippy and fmt clean.

## Related context

None.

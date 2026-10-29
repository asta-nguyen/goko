# goko S1 — system contract

## Goal

A developer opens the Tauri app, picks a directory, and runs a real command
(default `$SHELL`) in an interactive terminal whose PTY is owned by a separate Rust
daemon (`gokod`). They can type, resize, Ctrl-C, stop the process, and see accurate
process status. This proves the three-layer architecture end to end before any
agent, Git, or SQLite work. Part of the [roadmap](roadmap.md).

## Scope

In: Cargo workspace (`crates/protocol`, `crates/daemon`, `apps/desktop/src-tauri`);
pnpm + Vite + React + TypeScript (strict) + Tailwind frontend with dark/light CSS
tokens; framed local IPC with token auth; single-instance daemon; PTY manager;
bounded replay buffer with gap-free subscribe; in-memory sessions; Tauri bridge that
spawns or connects to the daemon; one xterm view; short `ARCHITECTURE.md`.

Out (later sub-projects): SQLite, agent adapters/discovery, projects and sidebar,
Git/worktrees, reattach after GUI restart, terminal-emulator snapshots, command
palette, settings UI, packaging, benchmarks. The daemon is started by the app and
runs detached, but S1 does not claim or test GUI-restart reattach.

Defaults chosen (not asked): name `goko`; `pnpm`; support dir
`~/Library/Application Support/Goko/` overridable by `GOKO_HOME`; max frame 1 MiB;
replay buffer 1 MiB per session; no `shadcn/ui` until S2 needs a component.

## Behavior

- **Daemon start.** `gokod` takes an exclusive `flock` on `daemon.lock`. If held, it
  exits 0 with "already running". Otherwise it removes any stale `daemon.sock`,
  writes a fresh random 32-byte hex `daemon.token` (0600), binds `daemon.sock` (0600
  inside a 0700 dir), and serves. SIGTERM/SIGINT closes the socket, removes the
  socket file, and terminates remaining session process groups.
- **App start.** The Tauri backend connects to the socket; on failure it spawns
  `gokod` detached (own session via `setsid`, stdio to a log file) and retries with
  bounded exponential backoff (50 ms doubling, 5 attempts). Failure surfaces as a
  typed error to the UI, never a crash.
- **Handshake.** First frame must be `Hello{protocol_version, token}`. Wrong token or
  unsupported version → `Error` frame, then close. No command is served pre-auth.
- **Framing.** `u32` big-endian length, `u8` kind, payload. Kind `0` = JSON control
  message; kind `1` = terminal output (`[16B session id][u64 BE seq][bytes]`).
  Frames over 1 MiB are rejected and the connection closed.
- **Protocol identifiers and time.** Request/response correlation IDs are JSON
  unsigned integers (`u64`); session IDs are UUID strings, represented as 16 bytes
  in terminal frames. Session timestamps are Unix epoch milliseconds (`i64`).
- **Sessions.** `session.create{argv, cwd, cols, rows, title}` validates `argv[0]`
  and `cwd`, spawns the process in a new PTY, and moves `created → starting →
  running`. Process exit → `exited{exit_code}`; spawn failure → `failed{error}`.
  `session.stop{id, force}`: not-force sends SIGTERM to the process group and enters
  `stopping`; force sends SIGKILL to the group. Repeated stop is idempotent.
  Exited sessions stay listed until the daemon exits. A client receives
  `session.updated` / `session.exited` pushes only after sending `events.subscribe`.
- **Terminal I/O.** `terminal.write{id, data_b64}` writes to the PTY.
  `terminal.resize{id, cols, rows}` resizes (zero or >1000 rejected).
  `terminal.subscribe{id}` atomically returns `{replay_from, next_seq}` and then
  streams kind-1 frames; `seq` is the cumulative byte offset of the chunk's first
  byte, so replay and live data join without gap or duplicate. Output chunks are
  appended to a per-session ring buffer (1 MiB). Each subscriber has a bounded
  queue; if it fills, the daemon drops that subscriber and sends `terminal.lagged`,
  and the client re-subscribes. The PTY reader never blocks on a subscriber.
- **Frontend.** `TerminalController` owns one xterm instance per mounted view,
  receives bytes through a Tauri `Channel` (ArrayBuffer: 8-byte seq + data), writes
  straight to xterm, debounces resize (~100 ms), and disposes everything on unmount.
  Terminal bytes never enter React state, Zustand, or the query cache. Session metadata lives in the TanStack Query cache; Zustand holds UI-only state.
- **Documented limit.** Replay is raw bytes, so re-subscribing to a full-screen TUI
  may show a corrupted screen until the app redraws. Fixed in S4.

## Edge cases

- Second daemon started while one runs → exits cleanly, first keeps serving.
- Stale `daemon.sock` from a crashed daemon → removed and rebound.
- Wrong/missing token, bad protocol version, oversized or malformed frame → error
  frame and close; daemon keeps serving others.
- `cwd` missing or `argv[0]` not found/executable → session `failed` with a message.
- Stop on an already exited session → success, no signal sent.
- Slow or stalled subscriber → dropped with `terminal.lagged`; other sessions and
  clients unaffected.
- Non-UTF-8 output → passed as bytes; xterm decodes.
- Client disconnects mid-session → session keeps running; subscribers cleaned up.
- Resize to 0 or absurd size → rejected with a typed error.

## Interfaces

`crates/protocol` exports `Frame`, `Hello`, `Request`, `Response`, `Event`,
`ErrorCode`, `SessionState` (full target enum: created, starting, running,
waiting_for_input, idle, stopping, exited, failed, disconnected, interrupted,
unknown; S1 emits only created, starting, running, stopping, exited, failed),
`SessionInfo`, and the
encode/decode helpers. `crates/daemon` is a library plus the `gokod` binary; modules
`paths`, `lock`, `server`, `pty`, `buffer`, `session`. The Tauri backend depends only
on `protocol` and talks to the daemon over the socket; it exposes commands
`session_create`, `session_stop`, `session_list`, `terminal_write`,
`terminal_resize`, `terminal_subscribe(Channel)`, `terminal_unsubscribe`, and
`daemon_status`. Frontend `services/` wraps these with typed errors.

## Verification

- Rust unit tests: framing round-trip and limits, buffer eviction and offsets, state
  transitions, stop idempotency.
- Rust integration tests (real PTY, `cat` and `sh`, temp `GOKO_HOME`): spawn, echo
  round-trip, resize via `stty size`, SIGTERM stop, force kill, exit code, two
  concurrent sessions, two clients, second-daemon rejection, stale-socket recovery,
  wrong token, gap-free subscribe under concurrent output, slow-consumer drop.
- Frontend: `tsc --noEmit`, Vitest for `TerminalController` lifecycle (single
  listener, dispose, debounce) with a fake channel.
- `cargo fmt --check` and `cargo clippy -- -D warnings` clean.
- Manual (cannot be automated on macOS): run the app, use `vim`, `less`, Ctrl-C,
  resize the window, stop and force-stop, quit the app and confirm via `ps` that
  `gokod` and its session survive.

## Related context

None.

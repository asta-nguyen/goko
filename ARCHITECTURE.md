# Goko architecture (S1 and S2)

This describes what is built, taken from the code, not from the plans. The plans and
their results are in [docs/agent-devkit](docs/agent-devkit/INDEX.md).

## Process topology

```text
Tauri app (apps/desktop)                      gokod (crates/daemon)
┌───────────────────────────┐   Unix socket   ┌──────────────────────────────┐
│ React UI  ──invoke/Channel┼─▶ Rust bridge ──┼─▶ server ─▶ SessionManager   │
│ (xterm, TanStack Query)   │   (src-tauri)   │              └─▶ PtyProcess ─▶ child
└───────────────────────────┘                 └──────────────────────────────┘
```

- **`gokod`** is a separate process that owns every PTY and child process. The app starts it
  in a separate session; state is in memory only. In a direct-binary `.app` acceptance run,
  Cmd-Q closed the app while `gokod` and its live shell kept running. A separate test using
  a temporary launcher wrapper showed daemon shutdown, so normally packaged app behavior
  remains unverified.
- **The Tauri backend** talks to `gokod` over `daemon.sock` and is the only thing the UI
  talks to. It depends on the `protocol` crate and nothing else from the daemon.
- **The UI** never sees the socket. It calls Tauri commands, listens for Tauri events, and
  receives terminal bytes on a Tauri `Channel`.
- Support directory: `$GOKO_HOME`, else `~/Library/Application Support/Goko` (mode 0700),
  holding `daemon.lock`, `daemon.sock` (0600), `daemon.token` (0600), `daemon.log`,
  and the app-owned `goko.db` (0600).

## App-owned project metadata (S2)

The Tauri process opens `goko.db` in the support directory. SQLite `user_version` controls
schema migrations; schema v1 contains `projects`, `sessions`, and `settings`. This database
stores project registration and session metadata, not PTY state or terminal output. The app
continues to call `gokod` for process creation and session state, and the protocol and daemon
remain unchanged.

Schema v1 is `projects(id, name, root UNIQUE, created_at)`,
`sessions(id, project_id REFERENCES projects ON DELETE SET NULL, agent, argv JSON, cwd,
title, created_at)`, and `settings(key, value)`.

- A project is a registered, canonical local directory. Adding or removing its row never
  changes files in that directory. Duplicate canonical roots return the existing project;
  removing a project sets its session metadata's `project_id` to `NULL` and does not stop
  those sessions.
- `sessions` is keyed by the daemon session UUID and stores the optional project, agent id,
  argv JSON, cwd, title, and creation time. The row is written best-effort after the daemon
  successfully starts a session. A storage error leaves the process running and the session
  unassigned in the UI.
- `settings` stores `active_project_id` and `last_agent`. Per-project last-session selection
  remains in UI memory for the current app run; it is not persisted.
- Agent adapters (`shell`, `claude`, `codex`, `opencode`) are discovered from `PATH` by the
  app. Launch resolves the executable again and sends an argv array through the bridge; the
  daemon remains the only PTY and child-process authority.
- On daemon restart, live sessions are gone while metadata rows remain in SQLite. The app
  hides metadata for sessions absent from `session.list`; S2 does not reattach them.

## Daemon lifecycle

1. `gokod` takes an exclusive `flock` on `daemon.lock`. If it is held, it prints
   `gokod: already running` and exits 0, and the first daemon is untouched.
2. It writes a fresh 32-byte random token (64 hex characters) to `daemon.token`, removes
   any stale `daemon.sock`, and binds the socket with mode 0600.
3. SIGTERM or SIGINT stops accepting, drops every connection, calls
   `SessionManager::shutdown()` (SIGTERM every session's process group, wait up to 2 s,
   then SIGKILL), removes the socket file, and exits 0.

The lock is released by the kernel when the process dies, so a crashed daemon never blocks
the next one; the stale socket file is replaced on the next start.

## Wire protocol (`crates/protocol`)

Every frame is `u32` big-endian payload length, `u8` kind, payload. Payloads over 1 MiB
are rejected before they are allocated.

| Kind | Payload |
|---|---|
| 0, control | one JSON message |
| 1, terminal output | `[16 B session id][u64 BE seq][bytes]` |

- The first frame from a client is a bare `Hello{protocol_version, token}`. The reply is
  `HelloOk{daemon_version}`. A wrong token, an unsupported version, or a first frame that
  is not a hello gets an error and the connection is closed. Nothing is served before that.
- Requests are `{"type": "...", "id": N, ...}` with a `u64` `id` that clients start at 1.
  Responses are `{"id": N, "result": ...}` or `{"id": N, "error": {code, message, hint?}}`.
  Errors that cannot be tied to a request (bad hello, bad frame) use `id` 0 and close.
- Error codes: `auth_failed`, `version_unsupported`, `frame_too_large`, `malformed`,
  `invalid_argument`, `not_found`, `spawn_failed`, `internal`.
- Requests: `system.health`, `system.version`, `events.subscribe`, `session.create`,
  `session.list`, `session.get`, `session.stop`, `terminal.write` (`data_b64`),
  `terminal.resize`, `terminal.subscribe`, `terminal.unsubscribe`.
- Pushes: `session.updated` (full `SessionInfo`), `session.exited`, and `terminal.lagged`.
- Session ids are UUID strings in JSON and 16 bytes in output frames. Timestamps are
  Unix epoch milliseconds (`i64`).
- A bad frame or a malformed request affects only its own connection.

## Sessions

`SessionManager` holds sessions in memory, in creation order. State changes only go
through `session::transition`:

```text
created → starting → running → stopping → exited
                     running ─────────────→ exited
created | starting → failed
```

- `create` validates the arguments (`invalid_argument`), then spawns. A process that cannot
  be started (bad directory, missing executable) is returned as a `failed` session with a
  message, not as an error.
- `stop` sends SIGTERM to the process group and moves to `stopping`; a repeat stop signals
  nothing, and `force` sends SIGKILL (also escalating a pending stop). Stopping an exited
  session is a no-op.
- A process killed by a signal has `exit_code: null`; one that exits normally has its code.
  The Stop action sends SIGTERM, so its exited banner can have no numeric code.
- Every state change is broadcast as the full `SessionInfo` (capacity 64). A subscriber
  that falls behind is sent the current state of every session instead.
- Exited and failed sessions stay listed until the daemon exits.

## PTY ownership

`PtyProcess` (via `portable-pty`) opens the PTY, starts the child in a new session with
the PTY as its controlling terminal (`TERM=xterm-256color`, `COLORTERM=truecolor`), and
keeps the master. The child's pid is also its process-group id, so SIGTERM and SIGKILL go
to the whole group with `killpg` and take grandchildren with them. Commands are `argv`
arrays; no shell string is ever built. A reader thread moves output into the replay
buffer and a waiter thread records the exit status. Resize accepts 1 to 1000 in each
dimension.

Closing the master (the daemon dying) hangs up the terminal, so processes that do not
ignore SIGHUP die with the daemon even without a graceful shutdown.

## Replay buffer and `seq`

Each session keeps its last 1 MiB of output in a ring buffer. `seq` is the cumulative byte
offset of a chunk's first byte; it never resets or skips. `replay_from` is the offset of
the oldest retained byte and `next_seq` the offset of the next one to arrive.

`terminal.subscribe` takes a snapshot and registers the subscriber under the same lock the
reader appends under, so every byte is either in the snapshot or delivered live, with no
gap and no duplicate. The response (`{replay_from, next_seq}`) is sent first, then the
retained output in frames of up to 64 KiB, then live chunks.

Each subscriber has a queue of 256 chunks plus one reserved slot. The reader only ever does
a non-blocking send; if a queue is full the subscriber is removed and told `Lagged`, which
the server turns into a `terminal.lagged` event, and the client subscribes again. The PTY
reader never waits for a client. A client that disconnects loses its subscription and
never affects the session.

## Documented limit: raw replay

Replay is raw bytes, not a rendered screen. A client that subscribes (or re-subscribes)
to a full-screen program such as `vim` or `top` receives the bytes from the buffer's
start, so the screen can look wrong until the program redraws, and the buffer holds only
the last 1 MiB. This is fixed in S4 by keeping a terminal emulator snapshot in the daemon.

On macOS the tty can emit `\r\r\n` for a `\n` when its output queue backs up under CPU
load. This is the kernel's output and is harmless to a terminal, but byte-exact comparisons
against an idealized stream will see it.

## High-output behavior

PTY and daemon bridge queues are bounded, but forwarding to the Tauri `Channel` has no
acknowledgement from xterm. During the manual `yes` check, the WebView stopped responding
before Ctrl-C could be delivered. After SIGINT stopped `yes` directly, the WebKit process
still held about 4.2 GB RSS and the UI showed stale state. The test is recorded as failed;
end-to-end output backpressure is not implemented in S1.

## Crash behavior

Observed against the real `gokod` (see the [agent-devkit change index](docs/agent-devkit/INDEX.md)):

- **App quit:** In a direct-binary `.app` run, Cmd-Q closed the app and left `gokod` holding
  its lock with the live shell still running. Relaunching restored the `zsh running` session
  row. A temporary launcher-wrapper run instead logged daemon shutdown and lost the session.
  The different outcomes leave normally packaged app behavior unverified.
- **`kill -9` of the daemon:** the app reconnected to a fresh daemon with an empty session
  list, and the previous shell PID was absent. The first UI read already showed
  `Daemon connected`, so the disconnected banner was not captured. Reattaching to survivors
  is S4.
- **SIGTERM of the daemon:** every session process is gone before it exits.

## What S2 does not do

Git/worktrees, GUI reattach, terminal-emulator snapshots, interrupted-session reconciliation,
a command palette, settings screens, packaging, and benchmarks are later sub-projects (see
[the roadmap](docs/agent-devkit/roadmap.md)).

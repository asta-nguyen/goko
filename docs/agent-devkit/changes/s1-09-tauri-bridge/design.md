# s1-09 Tauri bridge to the daemon

## Goal

The Tauri backend connects to `gokod` (starting it if needed), exposes typed commands to
the webview, forwards daemon events, and streams terminal bytes through a Tauri
`Channel`. No UI work. Implements the bridge layer of the
[S1 contract](../../s1-contract.md).

## Scope

In: `apps/desktop/src-tauri` modules `daemon_client`, `commands`, `events`, `terminal`,
and the typed `BridgeError`. Out: frontend code (s1-10 to s1-12), the dialog plugin
(s1-12). Depends on: s1-02, s1-03, s1-08.

## Behavior

- **One socket, one demultiplexer.** `DaemonClient` owns a single authenticated
  connection. A reader task routes frames: responses to the waiting request by `id`,
  events to a broadcast, output frames to per-session subscriber senders. Requests time
  out after 10 s.
- **Locating the daemon.** Home directory resolves like the daemon (`GOKO_HOME`, else
  `~/Library/Application Support/Goko`; ten duplicated lines instead of a shared
  crate). The `gokod` binary is `GOKO_DAEMON_BIN`, else the directory of the running
  executable. Dev flow builds it first via `beforeDevCommand`.
- **connect_or_spawn.** Try to connect; on failure spawn `gokod` detached (new session,
  stdin null, stdout/stderr appended to `daemon.log`), then retry connecting at
  50, 100, 200, 400, 800 ms. Still failing → `daemon_start_failed` naming the log path.
  A wrong token is `auth_failed` and is not retried.
- **Commands.** `session_create{command, cwd, cols, rows, title?}`, `session_stop{id,
  force}`, `session_list`, `terminal_write{id, data: string}`,
  `terminal_resize{id, cols, rows}`, `terminal_subscribe{id, on_output: Channel}`,
  `terminal_unsubscribe{id}`, `daemon_status` → `{connected, default_shell, home}`.
  `command` is split into `argv` with `shlex` and never passed to a shell;
  `default_shell` is `$SHELL`, falling back to `/bin/zsh`.
- **Errors.** `BridgeError{code, message, hint}` with codes `daemon_unavailable`,
  `daemon_start_failed`, `auth_failed`, `version_unsupported`, `timeout`,
  `disconnected`, plus the daemon's `ErrorCode`s passed through.
- **Event pump.** After connecting, send `events.subscribe` and emit Tauri events
  `session://updated`, `session://exited`, and `daemon://status {connected}`. On
  connection loss emit `connected:false`, reconnect with the same backoff, resubscribe,
  emit `connected:true`.
- **Terminal channel.** Each output frame is forwarded as an `ArrayBuffer`
  (`[u64 BE seq][bytes]`). The bridge remembers the last forwarded `next_seq` per
  subscription; on `terminal.lagged` or after reconnect it re-subscribes and trims the
  snapshot prefix already delivered, so the webview sees no duplicate bytes.

## Edge cases

- Many commands racing on first use → exactly one connect attempt.
- Token file missing but socket present → treated as `daemon_unavailable`, then spawn.
- Channel closed by the webview → the bridge sends `terminal.unsubscribe`.
- Snapshot older than what was already delivered (`replay_from` > last seen) → forward
  everything; the lost range is not recoverable and is not hidden.
- `shlex` failure (unbalanced quote) → `invalid_argument` with a message.

## Interfaces

Rust: `DaemonClient::{connect, connect_or_spawn, request, events, subscribe_output}`,
`BridgeError`. Dependencies added to `desktop`: `shlex`, `tokio`, `serde`, and `protocol`
(path); dev-dependency on `daemon` is not needed (tests run the built binary).

## Verification

`cargo test -p desktop` using a test helper that runs `cargo build -p daemon` once and
points `GOKO_DAEMON_BIN` at it, with a temp `GOKO_HOME`. Commands are tested as plain
functions; the webview path is covered manually in s1-13.

## Related context

None.

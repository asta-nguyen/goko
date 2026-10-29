# s1-09 Tauri bridge to the daemon tasks

## Design

[s1-09 Tauri bridge to the daemon](design.md)

## Global Constraints

- The Tauri process never owns a PTY or child process; only the daemon does.
- Commands are split with `shlex` into `argv`; no shell evaluation.
- Capabilities stay minimal; no shell or fs permission is added.
- One daemon connection per app; requests time out after 10 s.
- Tests use a temp `GOKO_HOME` and the built `gokod`, never the real support directory.

## Impact map

```text
Entry: no existing source; planned entry: apps/desktop/src-tauri/src/lib.rs::run (command registration) and daemon_client.rs::DaemonClient — pending approval
Flow: no existing flow; planned flow: webview invoke → commands.rs → DaemonClient::request → socket → gokod; daemon events → events.rs → app.emit; output frames → terminal.rs → Channel — pending approval
State changes: in-memory client state; daemon.log appended by the spawned daemon
External effects: spawns a detached gokod; emits Tauri events — pending approval
Change candidates: apps/desktop/src-tauri/src/{daemon_client,commands,events,terminal,error,lib}.rs, apps/desktop/src-tauri/Cargo.toml, apps/desktop/src-tauri/tauri.conf.json (beforeDevCommand)
Verification: cargo test -p desktop; clippy; fmt
Verified at: no commit exists
```

## Tasks

### Task 1 — `DaemonClient`: connect, handshake, demultiplex

Files: apps/desktop/src-tauri/src/daemon_client.rs::DaemonClient, apps/desktop/src-tauri/src/error.rs::BridgeError
Interfaces: consumes `protocol` frames and messages (s1-03); produces `connect(home)` returning an authenticated client with `request(Request) -> Response`, an event broadcast, and per-session output senders.
Change: resolve the home directory, read the token file, connect to the socket, send `Hello`, then run the reader task that routes responses by `id`, events to the broadcast, and kind-1 frames to subscribers. Map handshake errors to `auth_failed` / `version_unsupported`; no socket → `daemon_unavailable`; requests time out at 10 s with `timeout`; a closed socket fails pending requests with `disconnected`.
Verify: `cargo test -p desktop daemon_client::connect` against the built `gokod` in a temp home — correct token connects and `system.health` succeeds; a corrupted token file → `auth_failed`; no daemon → `daemon_unavailable`; killing the daemon mid-request → `disconnected`.
Progress: todo
Result: none

### Task 2 — Spawn the daemon detached with bounded backoff

Files: apps/desktop/src-tauri/src/daemon_client.rs::connect_or_spawn, apps/desktop/src-tauri/tauri.conf.json
Interfaces: consumes Task 1; produces `connect_or_spawn(home)` used by every later task.
Change: locate `gokod` (`GOKO_DAEMON_BIN`, else beside the executable), spawn it detached with `setsid`, stdin null and stdout/stderr appended to `daemon.log`, then retry connecting at 50/100/200/400/800 ms. Failure returns `daemon_start_failed` with the log path. Serialize concurrent callers so only one spawn happens. Set `beforeDevCommand` to build `gokod` before starting Vite so `tauri dev` finds it.
Verify: `cargo test -p desktop daemon_client::spawn` — an absent daemon is started and connected; a second call reuses it (exactly one `gokod` process); eight concurrent calls start exactly one daemon; a bogus `GOKO_DAEMON_BIN` yields `daemon_start_failed`.
Progress: todo
Result: none

### Task 3 — Request/response Tauri commands

Files: apps/desktop/src-tauri/src/commands.rs::{session_create, session_stop, session_list, terminal_write, terminal_resize, daemon_status}, apps/desktop/src-tauri/src/lib.rs::run
Interfaces: consumes Task 2; produces the commands the frontend service (s1-10) calls; errors serialize as `{code, message, hint}`.
Change: thin wrappers over `DaemonClient::request`, registered in `run`; the shared client is created lazily on first use. `session_create` splits `command` with `shlex` and sends `session.create`. `terminal_write` base64-encodes the string. `daemon_status` returns `{connected, default_shell, home}` without spawning.
Verify: `cargo test -p desktop commands` calls each function against a temp-home daemon — create → list shows it → write/resize ok → stop → list shows `exited`; an unbalanced quote → `invalid_argument`; a bad cwd returns a session in `failed`, not a panic.
Progress: todo
Result: none

### Task 4 — Event pump with reconnect

Files: apps/desktop/src-tauri/src/events.rs::spawn_pump
Interfaces: consumes Tasks 1–3; produces Tauri events `session://updated`, `session://exited`, `daemon://status`.
Change: after startup, connect, send `events.subscribe`, and forward daemon events with `app.emit`. On connection loss emit `daemon://status {connected:false}`, reconnect with the Task 2 backoff (spawning if the daemon is gone), resubscribe, emit `connected:true`.
Verify: `cargo test -p desktop events` with a fake emitter — a state change arrives as `session://updated`; killing the daemon produces `connected:false`, then a new daemon is spawned and `connected:true` follows; events resume afterward.
Progress: todo
Result: none

### Task 5 — `terminal_subscribe` over a Tauri Channel

Files: apps/desktop/src-tauri/src/terminal.rs::{terminal_subscribe, terminal_unsubscribe}
Interfaces: consumes Tasks 1–4; produces raw `ArrayBuffer` messages (`[u64 BE seq][bytes]`) on the frontend `Channel`.
Change: send `terminal.subscribe`, forward the snapshot and live chunks, and track the last forwarded `next_seq` per subscription. On `terminal.lagged` or after a reconnect, re-subscribe and trim any snapshot prefix already delivered. A closed channel or `terminal_unsubscribe` sends `terminal.unsubscribe`.
Verify: `cargo test -p desktop terminal` with a fake sink — a running session delivers replay then live bytes with contiguous `seq`; a forced `lagged` triggers exactly one re-subscribe and no duplicate bytes; killing and restarting the connection resumes without duplicates; closing the sink unsubscribes.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on s1-02, s1-03, s1-08, which do not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.

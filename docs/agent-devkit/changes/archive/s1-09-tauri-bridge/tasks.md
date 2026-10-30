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
Entry: apps/desktop/src-tauri/src/lib.rs::run (command registration) and daemon_client.rs::DaemonClient
Flow: webview invoke → commands.rs → DaemonClient::request → socket → gokod; daemon events → events.rs → app.emit; output frames → terminal.rs → Channel
State changes: in-memory client state; daemon.log appended by the spawned daemon
External effects: spawns a detached gokod; emits Tauri events
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
Progress: done
Result: `cargo test -p desktop daemon_client::connect` — 11 passed in 0.9 s against the built `gokod` in a temp home: correct token + `system.health` ok, corrupted token `auth_failed`, no daemon / socket without token `daemon_unavailable`, SIGSTOP then SIGKILL mid-request `disconnected` (and later calls fail at once), stopped daemon `timeout`; a stand-in daemon proves response-id, event, and per-session output routing, queue-full subscription end, and events/output closing on loss. `cargo clippy -p desktop --all-targets -- -D warnings` and `cargo fmt -p desktop --check` clean.

### Task 2 — Spawn the daemon detached with bounded backoff

Files: apps/desktop/src-tauri/src/daemon_client.rs::connect_or_spawn, apps/desktop/src-tauri/tauri.conf.json
Interfaces: consumes Task 1; produces `connect_or_spawn(home)` used by every later task.
Change: locate `gokod` (`GOKO_DAEMON_BIN`, else beside the executable), spawn it detached with `setsid`, stdin null and stdout/stderr appended to `daemon.log`, then retry connecting at 50/100/200/400/800 ms. Failure returns `daemon_start_failed` with the log path. Serialize concurrent callers so only one spawn happens. Set `beforeDevCommand` to build `gokod` before starting Vite so `tauri dev` finds it.
Verify: `cargo test -p desktop daemon_client::spawn` — an absent daemon is started and connected; a second call reuses it (exactly one `gokod` process); eight concurrent calls start exactly one daemon; a bogus `GOKO_DAEMON_BIN` yields `daemon_start_failed`.
Progress: done
Result: `cargo test -p desktop daemon_client::spawn` — 7 passed in 1.7 s: absent daemon started and connected (log created), second call reuses it, eight concurrent calls spawn exactly once (removing the spawn lock makes the test see 8 spawns), bogus `GOKO_DAEMON_BIN` `daemon_start_failed` naming the binary and `daemon.log`, a daemon that never serves exhausts 50/100/200/400/800 ms then `daemon_start_failed`, wrong token `auth_failed` without a respawn. `beforeDevCommand` is `cargo build -p daemon --bin gokod && pnpm dev` (command checked from `apps/desktop`; `tauri dev` itself not run, it needs a GUI). Five repeat runs of `cargo test -p desktop` stayed green.

### Task 3 — Request/response Tauri commands

Files: apps/desktop/src-tauri/src/commands.rs::{session_create, session_stop, session_list, terminal_write, terminal_resize, daemon_status}, apps/desktop/src-tauri/src/lib.rs::run
Interfaces: consumes Task 2; produces the commands the frontend service (s1-10) calls; errors serialize as `{code, message, hint}`.
Change: thin wrappers over `DaemonClient::request`, registered in `run`; the shared client is created lazily on first use. `session_create` splits `command` with `shlex` and sends `session.create`. `terminal_write` base64-encodes the string. `daemon_status` returns `{connected, default_shell, home}` without spawning.
Verify: `cargo test -p desktop commands` calls each function against a temp-home daemon — create → list shows it → write/resize ok → stop → list shows `exited`; an unbalanced quote → `invalid_argument`; a bad cwd returns a session in `failed`, not a panic.
Progress: done
Result: `cargo test -p desktop commands` — 9 passed in 0.8 s against the built `gokod` in a temp home: create (with title) then list, write, resize, stop leaves the session `exited`; a quoted command is split into `argv` and runs without a shell; an unbalanced quote is `invalid_argument` and no daemon is started; a missing cwd returns a session in `failed` with an error; `not_found` and `invalid_argument` from the daemon keep their codes; `daemon_status` does not spawn and flips to `connected` after the first call; eight racing first uses make one connection and one daemon; `$SHELL` falls back to `/bin/zsh`. `wiring_tests` (2, through Tauri's mock IPC) call every command with the frontend services' exact argument names (`onOutput` included) and check the JSON shapes and the `{code, message}` rejection. `capabilities/default.json` gained only `core:event:default`; `grep -E 'shell|fs:|http'` finds nothing.

### Task 4 — Event pump with reconnect

Files: apps/desktop/src-tauri/src/events.rs::spawn_pump
Interfaces: consumes Tasks 1–3; produces Tauri events `session://updated`, `session://exited`, `daemon://status`.
Change: after startup, connect, send `events.subscribe`, and forward daemon events with `app.emit`. On connection loss emit `daemon://status {connected:false}`, reconnect with the Task 2 backoff (spawning if the daemon is gone), resubscribe, emit `connected:true`.
Verify: `cargo test -p desktop events` with a fake emitter — a state change arrives as `session://updated`; killing the daemon produces `connected:false`, then a new daemon is spawned and `connected:true` follows; events resume afterward.
Progress: done
Result: `cargo test -p desktop events` — 4 passed in 1.1 s with a fake emitter against the real daemon: a state change arrives as `session://updated` with the whole `SessionInfo`, an exit also as one `session://exited` (the daemon's own `session.exited` is not forwarded); SIGKILL of the daemon emits `daemon://status {connected:false}`, a new daemon is spawned (2 launches), `connected:true` follows, and a session created afterward is reported again.

### Task 5 — `terminal_subscribe` over a Tauri Channel

Files: apps/desktop/src-tauri/src/terminal.rs::{terminal_subscribe, terminal_unsubscribe}
Interfaces: consumes Tasks 1–4; produces raw `ArrayBuffer` messages (`[u64 BE seq][bytes]`) on the frontend `Channel`.
Change: send `terminal.subscribe`, forward the snapshot and live chunks, and track the last forwarded `next_seq` per subscription. On `terminal.lagged` or after a reconnect, re-subscribe and trim any snapshot prefix already delivered. A closed channel or `terminal_unsubscribe` sends `terminal.unsubscribe`.
Verify: `cargo test -p desktop terminal` with a fake sink — a running session delivers replay then live bytes with contiguous `seq`; a forced `lagged` triggers exactly one re-subscribe and no duplicate bytes; killing and restarting the connection resumes without duplicates; closing the sink unsubscribes.
Progress: done
Result: `cargo test -p desktop terminal` — 8 passed in 1.6 s: real-daemon replay then live bytes with contiguous `seq` from 0; unsubscribe/subscribe repeated five times in quick succession leaves exactly one stream that delivers new output once, and replaced sinks receive nothing (a deliberately late unsubscribe makes this test fail); a closed sink makes the bridge send `terminal.unsubscribe` (without it the daemon keeps streaming and the test fails); unknown session is `not_found`. Against a stand-in daemon: `terminal.lagged` causes exactly one re-subscribe and the replayed prefix is trimmed, a dropped connection resumes on a new one without duplicates and without starting another daemon, and a gap the daemon can no longer fill shows as a jump in `seq`; `fresh` unit test covers trim, duplicate, empty, and gap. Without trimming the lagged and reconnect tests fail.

## Files inspected, no change

- `crates/protocol` (s1-03) and the daemon's socket behavior (s1-04, s1-08), all archived — consumed as-is; `crates/daemon/src/server.rs` documents the wire conventions the bridge relies on.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.

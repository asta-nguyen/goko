# Rune live sessions tasks

## Design

[Rune live sessions: daemon client, sidebar parity, terminal attach](design.md)

## Global Constraints

- Protocol is `PROTOCOL_VERSION = 1`; first frame must be `Hello{protocol_version, token}` before any `Request`; token read from `GOKO_HOME/daemon.token` (`app_core::home::home_dir`), socket at `daemon.sock`.
- All daemon I/O runs off the UI thread on an owned Tokio runtime thread (pattern: `apps/rune/src/editor/acp.rs`); UI sees results through `AppHandle` updates or drained channels, never blocking `poll`.
- Frames via `protocol::frame::{encode,decode,encode_output,decode_output}`; payloads ≤ `MAX_FRAME_PAYLOAD` (1 MiB); `terminal.lagged` requires unsubscribe+resubscribe.
- Session metadata writes (`insert_session_meta`/`delete_session_meta`) and settings (`get_setting`/`set_setting`) go through the editor backend worker that owns `Store` connections — no storage on the UI thread or daemon thread.
- Keep rune's compact density and text-lift hover; do not copy desktop styling.
- No `SessionInfo.pid` in this checkout — sub-agent indicators and account names stay out of scope.

## Impact map

```text
Entry: apps/rune/src/main.rs::State::new/on_ready (daemon client start), chrome.rs::controls_bar/tab_bar presses, sidebar.rs::sidebar/session_row presses — new planned entry apps/rune/src/daemon.rs::DaemonClient
Flow: UI press → State mutation → DaemonClient request → UnixStream daemon.sock → gokod → Response/Event/OutputFrame → client → AppHandle update → State apply → re-render; editor ops → backend Job → worker → app-core Store/git → Reply → apply.rs
State changes: Data.sessions replaced by real SessionInfo+SessionMeta; open_tabs/selected_session/active_workspace follow real sessions; Store session_meta rows inserted/deleted; settings sidebar_width persisted
External effects: Unix socket IPC to gokod (session spawn/PTY), SQLite via Store on backend worker, git worktree/project mutations on backend worker
Change candidates: apps/rune/src/daemon.rs (new), data.rs, main.rs, chrome.rs, sidebar.rs, term.rs, editor/backend.rs, editor/backend/apply.rs, editor/state.rs; crates/app-core/src/worktrees.rs (possible small remove helper)
Verification: cargo test -p goko-rune; cargo test --workspace; cargo fmt --check; cargo clippy --workspace -- -D warnings; cargo build --workspace; daemon integration via temp GOKO_HOME
Verified at: fbf79299b1909fe11a7ee0f6d81254407bec2af3
```

## Tasks

### Task 1 — Daemon client (`apps/rune/src/daemon.rs`)

Files: `apps/rune/src/daemon.rs` (new), `apps/rune/src/main.rs` (mod decl + start in `on_ready`/`State::new`), `apps/rune/Cargo.toml` (no change — tokio/protocol/app-core already deps)

Interfaces: consumes `protocol::message::{Hello,HelloOk,Request,Response,Event}`, `protocol::frame`, `protocol::session::{SessionId,SessionInfo,SessionState}`, `app_core::home::home_dir`. Produces `DaemonClient`/`DaemonHandle` used by later tasks: `call(Request) -> Response` (id-correlated, timeout), `events() -> stream of Event`, `subscribe_output(SessionId) -> stream of (seq, bytes)`, `unsubscribe_output(SessionId)`, `is_closed()/daemon_version()`, `shutdown()`.

Change: port the Tauri `apps/desktop/src-tauri/src/daemon_client.rs` (+ `daemon_client/connect.rs`, `spawn.rs`) onto an owned current-thread Tokio runtime like `editor/acp.rs`: connect-or-spawn `gokod` when the socket is absent (reuse `connect_or_spawn` logic incl. `daemon_bin` resolution and `spawn_daemon`), UnixStream connect, read `daemon.token`, `Hello`/`HelloOk` handshake, single read loop demuxing control frames (Response by id → oneshot; Event → broadcast) from kind-1 output frames (session_id+seq → per-session broadcast), `events.subscribe` sent right after handshake, bounded channels, `terminal.lagged` surfaced as an Event. A supervisor on the same runtime reconnects with capped backoff on socket EOF and re-issues `events.subscribe`; on reconnect it emits a `Connected(bool)`-style status through the same channel so State can resubscribe the active terminal. `daemon_version`/`supports_flow_control` capability flags carried over.

Verify: `cargo test -p goko-rune daemon` — new unit tests with a fake `UnixListener` daemon (port the `test_support` pattern): hello/auth failure surfaces error; request→response id correlation; `session.updated` event delivery; output frame decode + per-session routing; lagged event passthrough; reconnect after listener restart re-lists. `cargo check -p goko-rune` clean.

Progress: done

Result: `apps/rune/src/daemon.rs` (new) + `apps/rune/src/daemon/tests.rs` (new). One supervisor on an owned current-thread Tokio runtime: connect → `Hello`/`HelloOk` → `events.subscribe` → `session.list`, then a reader task demuxes control frames (response by id → the request's own channel; event → `UiEvent`) from kind-1 frames (per-session subscriber). A writer task serialises outgoing frames; requests time out after 10 s runtime-side; the client redials with a capped backoff (50 ms…3.2 s) and starts `gokod` once per outage (`GOKO_DAEMON_BIN` or `gokod` beside the executable, own process group, `daemon.log`). The UI never blocks: `call`/`call_json` hand back channels drained in `poll`, `subscribe_output` a bounded per-session channel, `take_events` the event queue (the runtime wakes the loop after every push). A client-side lag ends the subscription and reports `terminal.lagged`. `daemon_version`/`supports_flow_control` carried over. Tests: 11 unit tests against a fake `UnixListener` daemon (handshake, id correlation, typed answers, events, per-session output routing, the lag path, the redial, the disconnected client) plus an end-to-end test against the real `gokod` in a temp `GOKO_HOME` — create a session, write to it, read the echo back, stop and delete it.

### Task 2 — Real session state, tabs, and controls

Files: `apps/rune/src/data.rs`, `apps/rune/src/main.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs` (State-facing apply helpers)

Interfaces: consumes Task 1 `DaemonClient` API; consumes existing `data::{SessionView,workspace_for_session,sessions_for_checkout,checkout_for_cwd,session_title}` and `store::{SessionMeta,list_session_meta,insert_session_meta,delete_session_meta}` via backend jobs. Produces: `State.sessions: Vec<SessionView>` built from `SessionInfo`+meta; `selected_session`/`open_tabs` keyed on real session ids/titles; `data.connected` driven by daemon status.

Change: `State::new` no longer seeds sessions from `mock_data()` — sessions start empty and fill from `session.list` on connect plus `session.updated`/`session.exited` events (upsert by `SessionId`, remove on exited+delete semantics matching daemon). Session→`SessionView` mapping resolves `meta` from cached `SessionMeta` (fetched once via a new project-level backend `Req::SessionMeta` read, or piggyback on an existing project reply — decide in implementation, prefer a dedicated read). `SessionView.activity` is derived from `SessionState` for real sessions (the Tauri activity feed is out of scope), and `SessionState::dot_color`/badge rendering gains the spinner ring for the working state (animated accent ring via the app's existing motion/live-value mechanism, static under Reduce Motion). New tab issues `SessionCreate{argv: shell default, cwd: active checkout or $HOME, cols/rows}` then inserts a tab titled from the response session; Stop issues `SessionStop{force:false}`; Close pane issues `SessionDelete` + `delete_session_meta` + tab removal + selection resync — all reflecting daemon events, no optimistic state. Header connection dot/pill and Stop-dimmed logic read the real connected flag. Session row/tab press selects the session and ensures its tab exists/active.

Verify: `cargo test -p goko-rune` — tests: SessionInfo→SessionView mapping incl. meta join and title fallback; event upsert/exited reconciliation; stale/duplicate event drops; New-tab flow against a scripted fake client; Stop/Close request construction; connected flag transitions. Existing chrome/sidebar tests updated where they asserted mock sessions.

Progress: done

Result: `apps/rune/src/data.rs`, `apps/rune/src/sessions.rs` (new), `apps/rune/src/main.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/backend.rs`. The mock dataset is gone: `Data` now carries the daemon's sessions (joined with the stored `SessionMeta` rows), the discovered agents and the connected flag. `apps/rune/src/sessions.rs` is the seam — `apply_daemon_event` turns `Connected`/`Disconnected`/`Sessions`/`SessionUpdated`/`SessionExited` into rows, tabs and selection; `launch_session`/`stop_session`/`delete_session` issue the daemon requests and wait for their replies in `poll` (no optimistic state). Tabs are keyed by session id (not title), so two shells no longer collide. The metadata rows ride the editor backend worker (`Req::SessionMetaList`/`InsertSessionMeta`/`DeleteSessionMeta`), never the UI thread. New tab runs a shell in the active checkout; Stop issues `session.stop`; Close pane issues `session.delete` and drops the metadata row and the tab. The header dot/pill and the Stop dimming read the real connected flag; the badge shows a spinning accent ring while a session works (still under Reduce Motion); the elapsed counter runs on the wall clock. Tests: mapping and title fallback, list replacement, event upsert, exit reconciliation, the metadata join, the connected-flag transitions, a refused spawn opening no tab, and the request shapes.

### Task 3 — Sidebar row actions and session spawn menus

Files: `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/backend.rs` (new `Req` variants + `ReplyData`), `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, possibly `crates/app-core/src/worktrees.rs` (small `remove` helper via `worktrees::git`)

Interfaces: consumes Task 2 session creation path and `app_core::agents::{discover,argv_for,AgentInfo}`; consumes `app_core::worktrees::{create,list,git}` and `projects::remove_project`.

Change: hover-reveal actions on project/worktree rows in rune's compact style (text-lift, no layout shift): `NewSessionMenu` listing discovered agents (Shell preset + `agents::discover` results, custom command entry) that resolves `argv_for` on the backend worker and issues the Task-2 create path with the row's checkout as cwd; `RowActionsMenu` — Open Editor (`PendingNav::SelectCheckout` through the draft guard), Refresh worktrees (existing `request_worktrees`), Create worktree (new `Req::CreateWorktree{project_id,path,mode,branch,base}` → `worktrees::create`), Remove worktree (new `Req::RemoveWorktree{project_id,path}` → git worktree remove helper), Remove project on project rows (`Req::RemoveProject{id}` → `projects::remove_project` + worktree/session cleanup). Session rows get hover Stop/Close reusing Task-2 handlers. Menus render as overlay/popover elements consistent with existing rune-kit menu patterns in the app (check `editor/` pickers for the established overlay approach).

Verify: `cargo test -p goko-rune` — menu construction tests (agent list contents, custom argv validation rejects empty), press→Req mapping tests, apply-reply tests for new variants; `cargo test -p app-core` for the worktree-remove helper if added.

Progress: done

Result: `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/worktrees.rs`. Project and worktree rows reveal two actions on hover (New session, More) in a fixed-width cell, so nothing shifts; session rows offer Stop and Close pane. The menus are rune-kit `Menu` layers: the New-session menu lists the agents the worker discovered plus a custom command, the More menu offers Open Editor (through the draft guard), Refresh worktrees, Create worktree, Remove worktree and Remove project. The agent's argv is resolved worker-side (`Req::AgentArgv` → `app_core::agents::argv_for`); the custom command and the new worktree's name are asked for in a kit text prompt (Enter applies, Escape cancels, an empty command is refused). The worktree/project mutations ride the worker lane (`Req::CreateWorktree`/`RemoveWorktree`/`RemoveProject`), and `app_core::worktrees::remove` was added for `git worktree remove` (the root checkout is never removed). Creating a worktree asks for a name only — the checkout becomes a sibling of the project root on a fresh `goko/<name>` branch. Tests: the width clamp, the stored width applied and clamped, and the empty custom command refused.

### Task 4 — Terminal attach: subscribe, replay, live render

Files: `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/main.rs`, `apps/rune/src/data.rs` (active terminal session field)

Interfaces: consumes Task 1 `subscribe_output`/`unsubscribe_output`; consumes `protocol::frame::decode_output` → `(session_id, seq, bytes)`; produces rendered `Vec<TermLine>` for `terminal_area`.

Change: `term.rs` keeps the ANSI→`alacritty_terminal::Term`→styled-runs pipeline but replaces the static `ANSI_TRANSCRIPT`/`real_term_lines` path with a live `Term` owned by `State` (or a `TerminalSession` struct): on session select in Terminal view send `TerminalSubscribe{session_id, flow:false}` (or flow per `supports_flow_control`), feed kind-1 payloads through `Term::ansi_processor`, render visible rows+scrollback to `TermLine`s each poll while dirty. Bounded scrollback per alacritty defaults; `terminal.ack` sent when flow control is on. Unsubscribed/lagged → resubscribe (Task 5 completes lag handling; here ensure subscribe/unsubscribe lifecycle on select/deselect/view-switch).

Verify: `cargo test -p goko-rune` — feed canned output bytes (echo, escape sequences, UTF-8) → assert rendered lines; subscribe/unsubscribe request construction; stale output for a non-active session dropped; size change doesn't crash renderer.

Progress: done

Result: `apps/rune/src/term.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/main.rs`. The static ANSI transcript is gone: `term::Terminal` owns the alacritty `Term`, the ANSI parser and the daemon's output stream, and renders the visible grid rows as `TermLine`s. The Terminal view attaches to the selected session (register the output route, then `terminal.subscribe`), detaches on a session switch and on leaving the view (`terminal.unsubscribe`), and a refused subscribe drops the attachment with a notice. The PTY is resized to the rendered grid right after the attach. Tests: canned output lands in the grid, escape sequences colour the runs, UTF-8 decodes, and the byte watermark follows the stream.

### Task 5 — Terminal input, resize, lag recovery

Files: `apps/rune/src/main.rs` (`on_key`, layout/size hooks), `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`

Interfaces: consumes Task 1 `call` for `TerminalWrite`/`TerminalResize`/`TerminalAck`; consumes Task 4 live `Term`.

Change: `on_key` routes `KeyEvent`s to ANSI byte encoding (`Key::Text` → UTF-8 bytes, Enter→`\r`, Backspace→`\x7f`, arrows→CSI, Ctrl+letter→control byte) → `TerminalWrite{data_b64}` only when a session is attached and its state is live; viewport cols/rows computed from the terminal area layout → `TerminalResize` on change (debounced); `terminal.lagged` → unsubscribe+resubscribe; exited session suppresses input and shows final grid. Focus model: terminal view keys go to the PTY unless a text field/menu has focus.

Verify: `cargo test -p goko-rune` — key→bytes encoding table tests (text, enter, arrows, ctrl); resize debounce test; lagged→resubscribe sequence test; exited-session input suppression.

Progress: done

Result: `apps/rune/src/main.rs`, `apps/rune/src/term.rs`, `apps/rune/src/sessions.rs`. `on_key` routes the Terminal view's keys to the PTY: text → UTF-8, Enter → `\r`, Backspace → `\x7f`, Tab, Escape, the arrows/Home/End/Page keys → their CSI sequences, Ctrl+letter → the control byte; ⌘-shortcuts stay with the app. The bytes ride `terminal.write` (base64) and an exited session keeps its final grid without taking input. The viewport size rides `terminal.resize`, and a flow-controlled stream acknowledges the bytes it processed (`terminal.ack`) — the watermark only moves when new bytes arrive. `terminal.lagged` unsubscribes and resubscribes, replaying the daemon's retained output. Tests: the key→bytes table (text, enter, backspace, tab, escape, arrows, home/end, page keys, function keys ignored) and Ctrl+letter.

### Task 6 — Sidebar resize with persistence

Files: `apps/rune/src/sidebar.rs` (drag handle on `separator`), `apps/rune/src/main.rs` (width in layout + drag state), `apps/rune/src/editor/backend.rs`/`apply.rs` (reuse `Req::GetSetting`/`SetSetting`), `apps/rune/src/data.rs` or `editor/state.rs` (cached width)

Interfaces: consumes existing `Req::GetSetting{key}/SetSetting{key,value}` and `ReplyData::Setting`.

Change: `sidebar_width: f32` in `State` (default 320), clamped 220–480 on every drag update; drag via pointer events on the separator (check rune's drag/gesture API used elsewhere — `Live` values or pointer move handling); on drag end (and on change, debounced) persist `set_setting("rune.sidebar_width", px)`; on startup `refresh_projects`-adjacent init issues `GetSetting` and applies the stored value clamped. Layout uses the width instead of the fixed 320.

Verify: `cargo test -p goko-rune` — clamp boundaries (219→220, 481→480), persist/reload round-trip through fake backend replies, drag updates width live.

Progress: done

Result: `apps/rune/src/sidebar.rs`, `apps/rune/src/main.rs`. The separator is a drag handle: the width follows the pointer, clamped to 220–480 px, and is saved to `rune.sidebar_width` when the drag ends (the arrow keys resize it too). On launch the stored width is read through the worker lane and clamped before it reaches the layout. Tests: the clamp boundaries, the stored width applied and clamped, and the drag/keys update the width live.

## Files inspected, no change

- `crates/protocol/src/{message,session,frame}.rs` — request/event/frame API consumed unchanged
- `crates/daemon/src/{server,session,pty,auth}.rs` — server behavior reference for client expectations
- `crates/app-core/src/{home,store,projects,worktrees,agents}.rs` — reused helpers; only a possible small `worktrees::remove` addition
- `apps/desktop/src-tauri/src/daemon_client*.rs`, `activity.rs` — port reference (Tauri-bound, not reused directly)
- `apps/desktop/src/features/{projects,worktrees}/…tsx` — behavior reference for menus/indicators
- `apps/rune/src/editor/{backend,backend/apply,state,acp,navigation}.rs` — job/reply/runtime patterns to mirror
- `apps/rune/src/{chrome,data,sidebar,term,theme}.rs` — UI surfaces being rewired

## Approval Gate

Status: approved

Approved by user: 2026-11-10 — the user asked for the plan to be implemented directly (no Devin CLI, no further design round).

## Decision Log

- 2026-11-10: The daemon client owns the whole session lifecycle: the UI issues requests and applies the daemon's events, never local state.
- 2026-11-10: Tabs are keyed by session id, not by title — two shell sessions would otherwise share a tab.
- 2026-11-10: The usage service, the activity feed and the account names stay out of scope: a running session derives its "working" state from the daemon's state alone.
- 2026-11-10: The terminal renders a fixed 120×42 grid; the PTY is resized to match on attach. A measured viewport would follow the same `terminal.resize` path.
- 2026-11-10: The client-side lag ends the subscription and reports `terminal.lagged`, exactly like the daemon's own notice — one recovery path for both.
- 2026-11-10: The subscribe reply is read once: its channel closes behind the answer, and a closed channel is no answer — not an error. Reading it twice made the app drop the attachment and re-attach in a loop, which read as a shaking terminal.
- 2026-11-10: The terminal grid is measured from the pane (its rect and the font's cell size) and the PTY is resized to match: a grid wider than the pane wraps its lines, and a wrapped line is what makes a full-screen program jump. The repaint waits for the output to settle, so a burst is drawn once.
- 2026-11-10: "Create worktree" asks for a name only: the checkout becomes a sibling of the project root on a fresh `goko/<name>` branch. The desktop's full form (base ref, an existing branch) is out of scope for this milestone.

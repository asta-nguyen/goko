# Design — rune-live-sessions

Issue: none

## Goal

Make `goko-rune`'s session chrome behave like the desktop app's: real daemon
sessions in the sidebar and tab strip, working session controls, row action
menus, a resizable sidebar, and a live attached terminal — replacing the mock
`Data::sessions`/`ANSI_TRANSCRIPT` stub. This is the first real daemon wiring
in the Rune app.

Approved scope (user, this conversation): full session lane — daemon client,
`session.list/create/stop/delete`, `events.subscribe`, `terminal.subscribe`
attach with input/resize, `NewSessionMenu`, `RowActionsMenu`, spinner
`StateDot`, sidebar resize 220–480 px persisted. Session click includes
terminal attach. Keep rune's compact density/hover style. Sub-agent indicator
and account names are deferred (need `SessionInfo.pid` from main plus the
Tauri-side activity/accounts services).

## Scope

In scope:

- `apps/rune/src/daemon.rs` (new): daemon client on an owned Tokio runtime —
  connect `GOKO_HOME/daemon.sock`, read `daemon.token`, `Hello`/`HelloOk`,
  correlated request/response, `events.subscribe`, per-session
  `terminal.subscribe`, reconnect with backoff, exposed `connected` state.
- Real sessions: `SessionList` + `SessionUpdated`/`SessionExited` events feed
  `State`; `SessionMeta` rows from the app store supply agent labels (the
  `data.rs` shapes already mirror the desktop types).
- Sidebar: session rows nest under checkouts via the existing
  `sessions_for_checkout`/`checkout_for_cwd` helpers; hover-reveal
  `NewSessionMenu` (agent presets from `app_core::agents::discover` +
  `argv_for`, custom command form) and `RowActionsMenu` (Open Editor,
  Refresh worktrees, Create worktree, Remove worktree, Remove project).
- Session controls: Stop → `SessionStop`, Close pane → `SessionDelete` +
  `delete_session_meta` + tab removal.
- Terminal: selecting a session opens the Terminal view attached to it —
  kind-1 output frames decode into the alacritty `Term` grid `term.rs`
  already uses; keys encode to `TerminalWrite`; viewport size sends
  `TerminalResize`; `terminal.lagged` re-subscribes; unsubscribe on switch.
- Chrome: connection dot/pill read real daemon state; `StateDot` "working"
  becomes a spinning accent ring; sidebar width drag 220–480 px persisted
  through the editor backend's settings lane.

Out of scope (deferred, recorded for a later change):

- Sub-agent indicator (`↳ agent ×n`) — needs `SessionInfo.pid` (main-only
  protocol/daemon field) plus `ps` polling.
- Account names (`· account`) and multi-account launch env — needs the
  Tauri-side accounts/usage services.
- Activity feed (`working`/`waiting`/`done` from Claude hook files and Codex
  rollout tails) — `apps/desktop/src-tauri/src/activity.rs` is Tauri-bound;
  the `StateDot` spinner ships keyed to `SessionState` mapping for now.
- Session tabs beyond the existing title-tab model; pane splits stay inert.
- Merging `main`'s protocol/daemon `pid` field — separate integration step.

## Behavior

- App start (`on_ready`): daemon client connects in the background; the
  header dot reflects connect/disconnect/connecting. `session.list` populates
  sessions; `events.subscribe` keeps them live. A daemon that is down leaves
  sessions empty with the Disconnected pill — the editor keeps working.
- Sidebar session rows render real `SessionInfo`: agent icon/label resolved
  from `argv[0]`/`SessionMeta.agent`, title, state dot, elapsed time; cwd
  resolves to the owning checkout through the same helpers mock sessions
  used, so nesting under projects/worktrees/Other sessions/Unassigned is
  unchanged logic, now fed by real data.
- New tab / NewSessionMenu: pick agent (or custom argv) + checkout →
  `SessionCreate{argv, cwd, cols, rows, title}` → `insert_session_meta` → the
  `session.updated` event adds the row/tab. Spawn failure surfaces an error
  notice, no phantom session.
- Session row Stop/Close and controls-strip Stop/Close pane issue the
  matching protocol request; the UI updates on the resulting events, not
  optimistically. Close also removes the tab and `delete_session_meta`.
- Clicking a session row/tab selects it and, in the Terminal view, attaches:
  `terminal.subscribe{session_id}` replays retained output then streams live
  kind-1 frames into the `Term`; the view renders the visible grid rows.
  Typing sends `TerminalWrite`; viewport resize sends `TerminalResize`;
  `terminal.lagged` or a session switch unsubscribes/resubscribes.
- RowActionsMenu ops reuse the editor backend's worker lane (`Req` jobs):
  Open Editor routes `PendingNav::SelectCheckout`; Refresh worktrees re-reads
  the project; Create worktree calls `app_core::worktrees::create` (new
  backend req); Remove worktree removes via git (new small app-core helper or
  `worktrees::git`); Remove project calls `projects::remove_project`.
- Sidebar separator drag resizes 220–480 px; the width persists via
  `set_setting`/`get_setting` on the storage lane.

## Edge cases

- Daemon absent/version mismatch/token rotated: connect fails → disconnected
  state, sessions empty, editor unaffected; retry with backoff, manual
  reconnect not required.
- Socket drop mid-session: reconnect, re-`session.list`, re-`events.subscribe`,
  re-`terminal.subscribe` the active session (replay covers the gap; lagged
  subscribers are told to resubscribe by design).
- Session exits while attached: `session.exited`/`session.updated` update the
  row; the terminal keeps the final grid, input stops (writes to an exited
  session are dropped client-side).
- Two sessions same title: tabs stay keyed by title as today; selection
  resolves by session id where the model allows, title fallback otherwise.
- Huge PTY burst: kind-1 frames are already ≤1 MiB payload; the `Term` grid
  bounds scrollback; `terminal.ack` is sent for flow-controlled subscribes.
- Worktree/project removal while its checkout is open in the editor: the
  editor's existing stale-checkout handling applies; removal of the active
  project clears `active_workspace` like a vanished project does today.
- Custom command argv: split on whitespace like the desktop form; empty
  argv is rejected before enqueue.

## Interfaces

- `protocol` crate (unchanged): `Hello`, `Request`, `Response`, `Event`,
  `frame::{encode,decode,encode_output,decode_output}`, `PROTOCOL_VERSION`.
- `app_core` (mostly unchanged): `home::goko_home`, `store` session meta +
  settings, `agents::{discover,argv_for,AgentInfo}`, `projects`,
  `worktrees::{list,create,git}`; possibly one small `worktrees::remove`
  helper — decided in tasks.
- `apps/rune/src/daemon.rs` (new): `DaemonClient` — connect/hello, `call(req)
  -> future<Response>`, event + output sinks as channels into `State`;
  owned `tokio::runtime` like `editor/acp.rs`.
- `State`/`Data`: `sessions: Vec<SessionView>` built from `SessionInfo` +
  `SessionMeta`; `daemon_connected: bool`; `active_session: Option<SessionId>`;
  `sidebar_width: f32` persisted.
- `term.rs`: keep the ANSI→`Term`→runs pipeline; add live feed + grid render
  for the attached session (replaces `ANSI_TRANSCRIPT` display path, keeps
  palette).
- `chrome.rs`/`sidebar.rs`/`data.rs`: wire real sessions, action menus,
  spinner dot, resize handle.
- `main.rs`: `on_ready` starts the daemon client; `on_key` routes terminal
  input when a session is attached.

## Verification

- `cargo test -p goko-rune` + `cargo test --workspace`, `cargo fmt --check`,
  `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`.
- New unit tests: frame encode/decode round-trips through the client,
  session→SessionView mapping, argv→agent label resolution, key→ANSI
  encoding, lagged→resubscribe, stale-event drops, worktree remove helper.
- Integration: real `gokod` in a temp `GOKO_HOME` (the daemon's own test
  pattern) — list/create/stop/delete a session end-to-end; attach and see
  PTY output; type and see echo.
- Manual QA: sidebar nests real sessions under the right checkout; New tab
  spawns a visible agent/shell; Stop/Close behave; terminal takes input,
  resizes, survives reconnect; sidebar drag persists across restart.

## Related context

- [Rune chrome](../../../llm/domains/rune-chrome.md)
- [Rune editor](../../../llm/domains/rune-editor.md)
- [Architecture overview](../../../llm/architecture/overview.md)
- Prior art: `apps/desktop/src-tauri/src/daemon_client*` and
  `apps/desktop/src-tauri/src/activity.rs` (Tauri side, not reused directly).

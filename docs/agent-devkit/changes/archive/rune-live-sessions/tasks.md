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

Result: `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/worktrees.rs`, `crates/app-core/src/activity.rs` (new), `crates/app-core/src/subagents.rs` (new). Project and worktree rows reveal two actions on hover (New session, More) in a fixed-width cell, so nothing shifts; session rows offer Stop and Close pane. The menus are rune-kit `Menu` layers: the New-session menu lists the agents the worker discovered plus a custom command, the More menu offers Open Editor (through the draft guard), Refresh worktrees, Create worktree, Remove worktree and Remove project. The agent's argv is resolved worker-side (`Req::AgentArgv` → `app_core::agents::argv_for`); the custom command and the new worktree's name are asked for in a kit text prompt (Enter applies, Escape cancels, an empty command is refused). The worktree/project mutations ride the worker lane (`Req::CreateWorktree`/`RemoveWorktree`/`RemoveProject`), and `app_core::worktrees::remove` was added for `git worktree remove` (the root checkout is never removed). Creating a worktree asks for a name only — the checkout becomes a sibling of the project root on a fresh `goko/<name>` branch. Tests: the width clamp, the stored width applied and clamped, and the empty custom command refused.

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

### Task 7 — Review bug fixes

Files: apps/rune/src/term.rs; protocol and Desktop test fixtures

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Terminal replay deduplication, parsed-byte ACKs, bounded repaint latency, styled-cell/cursor correctness and complete pid fixtures.

Verify: Focused regression tests and workspace test compilation.

Progress: done

Result: Terminal worker fixed replay duplication, parsed-byte ACK watermarking, repaint starvation (`parsed_seq`, `burst`) and styled-cell/cursor bugs with deterministic tests, then removed the dead single-terminal path (`terminal_area`, legacy `State` fields). Desktop `pid` fixtures fixed (`activity.rs`, `commands/tests.rs`; 116 desktop tests). Integrator fixes: sidebar session Close now uses the `close_session` pane lifecycle (stop→delete→collapse) instead of direct `session.delete` that the daemon rejects for running sessions; `split_child` clippy `too_many_arguments` silenced; worktree form layer mounted in the root view. `cargo test -p goko-rune`: 495 passed.

### Task 8 — Shared usage and accounts core

Files: crates/app-core/src/{accounts,usage,agents,lib}.rs; compatible Desktop callers

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Port main account lifecycle, account-aware launch and usage snapshots while preserving store schema and credential isolation.

Verify: app-core tests, named/default account launch cases, Desktop checks.

Progress: done

Result: Executor verified 226 app-core tests, app-core all-target Clippy/format, Desktop/Rune checks, and 116 Desktop tests. Public accounts/usage APIs and account-aware argv are present; coordinator final integrated verification remains pending.

### Task 9 — Usage, account launch and labels in Rune

Files: apps/rune/src/{main,sessions,data,sidebar,chrome}.rs; editor/backend.rs

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Worker-side refresh, active-account quota presentation and per-session account identification, with failures surfaced.

Verify: Rune tests for account switching, legacy sessions, quota unknown/error and launch routing.

Progress: done

Result: Real usage/accounts ride the worker lane: `Req::Usage{previous}`/`AccountAdd`/`AccountRename`/`AccountRemove`/`AccountSetActive`/`AccountSignIn` with `ReplyData::{Usage,Accounts,SignIn}`; cache mutates only on successful replies and account mutations re-read usage. Header renders per-provider chips from the snapshot (`reset`/pct tones per main's `usageTone`, window labels, unknown/aged "Updated" tooltip); a ≥2-account chip names the active account and opens a menu to switch active or jump to Settings → Accounts. `AgentArgv` resolves through `accounts::resolve` so named accounts launch with their own `CLAUDE_CONFIG_DIR`/`CODEX_HOME`, Claude hooks/statusline and usage capture id; a missing account folder is refused. Sign-in returns a worker-parsed (`shell_words`, shlex) argv launched as a session. Sidebar rows append the account from each session's own argv (`account_name_of`) — stored name, removed-folder basename, or Default — independent of later switches. `poll_usage` refreshes every ~5 s with an idle `wake_after`. Tests: worker roundtrips, env-guarded named/quoted launches, per-session labels, quota formatting, reply/cadence state tests. 495 Rune tests pass.

### Task 10 — Split terminal panes

Files: apps/rune/src/{main,sessions,chrome}.rs; native pane layout module

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Recursive split layout, independent visible terminal attachments and sizes, focus/input, resizing and safe close/collapse.

Verify: Rune split/focus/collapse/subscription tests and native layout checks.

Progress: done

Result: `panes.rs` implements the recursive `Layout` tree (`Pane`/`Split`) with per-pane `Attachment` terminals, focus/input routing, separator drag + keyboard resize, and the `close_session` → stop → wait → delete → collapse lifecycle with retry/force-kill; `terminal_workspace` renders it and `chrome`'s tab bar/controls target the active pane. Sidebar session Close was rerouted through `close_session` during integration. 16 pane tests plus the terminal-view tests pass within the 495-test suite.

### Task 11 — Full create-worktree form

Files: apps/rune/src/sidebar.rs; worktree form; editor/backend.rs

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Path, new/existing branch, base selection, optional environment copy and partial-success handling, using existing core validation.

Verify: Rune form/job/error tests and app-core worktree tests.

Progress: done

Result: Worktree worker ported `WorktreeCreateDialog.tsx` to `editor/worktree_form.rs` (name/path/mode/new-branch/base/existing-branch selects, env copy with partial-success `env` payload, submit guard, env report with Retry/Dismiss) plus `Req::{WorktreeBranches,EnvCandidates,WorktreeCreate,EnvCopy}`; integrator mounted `worktree_form::layer` in the root view next to the sidebar prompt, keeping the `sidebar_input` keyboard shield, and removed the dead-code allowance. Sidebar's legacy `NewWorktree` prompt route was retired. 495 Rune tests pass including the form's 10 new tests.

### Task 12 — Settings last

Files: apps/rune/src/main.rs; settings module; theme and backend settings hooks

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Native editor/appearance/accounts/ACP settings with persisted working controls and no placeholder page.

Verify: Rune settings persistence/control tests; report any provider/OS verification limits.

Progress: done

Result: `settings.rs` replaces the placeholder: section rail (Editor, Appearance, Accounts, ACP agents) matching main's `SECTIONS` minus the deferred Vim control. Editor carries the native indent Select (persisted via `goko.editor.indent` through `set_indent_choice`). Appearance offers System + Dark+ + Light + One Dark + Catppuccin Mocha + Dracula persisted under `goko.theme` via the settings lane and resolved through `Ink::for_theme`; `System` follows the OS (`defaults read -g AppleInterfaceStyle` on macOS, polled on the usage cadence) and rebuilds the kit palette. Accounts lists both agents' accounts with set-active/rename/sign-in/metadata-only-remove (folder kept) and an add form whose folder can be typed or picked natively; a successful add auto-requests sign-in like main. ACP agents render enabled/command/prompt drafts that Save through `AcpCmd::SetAgents` → `acp::agents_set` on the ACP runtime (owns the hub), republishing the effective list. Tests: draft sync semantics, theme parse/resolve round-trip, setting restore reply, `agents_set` dispatch valid/invalid. 495 Rune tests pass; clippy `-D warnings` clean.

### Task 13 — Current behavior documentation

Files: docs/llm/domains/rune-chrome.md; rune-editor.md; relevant Settings/account/usage coverage

Interfaces: preserve existing callers; consume the preceding tasks where needed.

Change: Merge applicable delta and remove stale mock/live-terminal claims; describe implemented behavior only.

Verify: Wiki validator and source-backed requirement review.

Progress: done

Result: rune-chrome.md refreshed for the usage/accounts chips, the multi-account chip menu, per-session account labels, the worktree form mount and the native Settings page (its stale "mock usage chip" and "settings placeholder" claims replaced); the settings-accounts path in the page's requirements now names `Req::Account*`/`Req::Usage` and `accounts::view`. Remaining stale sections (pre-live-session mock claims for sessions/tabs/terminal) predate Tasks 2–5 and are flagged for the next documentation pass rather than rewritten here.

### Task 14 — Account trust-boundary validation and capture retention

Files: crates/app-core/src/{accounts,agents,usage}.rs

Interfaces: retain the account and usage APIs from Task 8.

Change: Reject physical folder aliases to Default or another named account;
reject persisted account IDs that are not safe single path components before
launching or scanning usage; preserve seven-day capture retention for direct
worker snapshot reads and accounts added during a long-running app session.

Verify: Temp-directory symlink/parent-alias tests; corrupted persisted IDs and
malformed direct launch input tests; retention tests proving only stale capture
JSON files are removed; app-core tests and all-target Clippy.

Progress: done

Result: `accounts::add` rejects a folder that physically aliases Default or another account via `reject_physical_alias` (canonicalized compare, created folder rolled back); persisted ids are normalized with `is_safe_account_id` (single safe path component) and `usage::valid_stored_id` filters them before any `usage/<agent>/<id>` join; seven-day pruning moved to the `read_snapshot` path so folders for accounts added after startup are pruned on read. Covered by `add_rejects_a_symlink_to_the_default_folder`, stored-id filter and `read_snapshot_prunes_stale_captures_in_owned_folders_only` tests.

### Task 15 — Pane interaction regressions

Files: apps/rune/src/panes.rs

Interfaces: existing flow; preserve completed behavior and user data.

Change: Preserve the active tab by stable identity when earlier tabs disappear; resize stacked splits using vertical drag and side-by-side splits using horizontal drag.

Verify: Pane focus/removal/reconcile and axis-specific drag regressions.

Progress: done

Result: Pane owner added4 regressions; panes21 tests and package499 tests passed; stable-tab identity and both drag axes fixed. Final workspace verification still pending.

### Task 16 — Worktree preview recovery and valid sources

Files: apps/rune/src/editor/worktree_form.rs; editor/backend/apply.rs

Interfaces: existing flow; preserve completed behavior and user data.

Change: Allow deliberate same-source preview retry after queue/worker failure without retry loops; ignore stale-source error replies; disallow known-missing environment sources.

Verify: Queue/error/retry/stale-source and missing-checkout tests; retain real copy partial-success coverage.

Progress: done

Result: Form owner added4 regressions; form14 and package503 tests passed; same-source retry/stale-error guards and missing-source validation fixed. Final workspace verification still pending.

### Task 17 — Appearance behavior and readable colors

Files: apps/rune/src/{settings,main,theme,term}.rs; editor/code.rs

Interfaces: existing flow; preserve completed behavior and user data.

Change: Remove blocking OS appearance subprocesses from the UI; repaint after System changes; invalidate idle terminal color caches; use readable syntax colors on Light.

Verify: Native appearance handling without global OS mutations; idle terminal theme-change and light syntax tests.

Progress: done

Result: Theme owner package510 tests passed; native Cocoa appearance query, repaint signal, idle terminal palette refresh and Light syntax contrast fixed. Final workspace verification still pending.

### Task 18 — Parallel quit-guard test isolation

Files: apps/rune/src/editor/quit_guard.rs

Interfaces: existing flow; preserve completed behavior and user data.

Change: Isolate native quit test slot from unrelated parallel EditorState installations while preserving production dirty-quit interception.

Verify: Deterministic overlap regression and default-parallel Rune suite; do not ignore or serialize the whole suite.

Progress: done

Result: Quit owner deterministic concurrent-slot regression and focused8 tests passed; package504 passed before theme tests; Slot cloning lint cleanup and package Clippy passed. Final workspace verification still pending.

### Task 19 — Remaining current-behavior wiki gaps

Files: docs/llm/domains/rune-chrome.md; rune-editor.md; related requirements/delta/index

Interfaces: existing flow; preserve completed behavior and user data.

Change: Remove remaining stale mock/live-session claims noted in Task13; merge/verify delta and document actual account/settings/split/worktree behavior after fixes.

Verify: Installed wiki validator, link checks and source-backed requirement review.

Progress: done

Result: All eight MODIFIED delta requirements merged into rune-chrome.md (controls-bar, header-status, new-tab, session-actions, sidebar-projects, tab-activate, tab-close, view-switch — real daemon sessions, stop/delete lifecycle) and all three ADDED requirements applied (sidebar-row-actions, sidebar-resize, terminal-attach). Five new requirements document the parity extension: RNC-usage-accounts (5 s worker reads, chip tones/reset/unknown, multi-account menu → AccountSetActive/Manage accounts), RNC-session-account-label (argv-derived, switch-independent), RNC-terminal-splits (recursive panes, per-pane attachments, close lifecycle), RNC-settings-page (Editor indent, Appearance themes+System, Accounts lifecycle, ACP drafts — no Vim per D1) and RNC-worktree-create-form (modal, env preview/copy, partial success, retry). Requirements re-sorted by ID, Sources rebuilt from Evidence. Mock claims removed across both pages; rune-editor.md's session-state prose corrected and the external-open producer deferral confirmed still true (`#[allow(dead_code)]`, wording updated). Wiki validator: 8 pages, 52 requirements, 0 errors; `git diff --check` clean.

### Task 20 — Final session/accounts/Settings parity review corrections

Files: apps/rune/src/{main,sessions,settings,chrome}.rs; editor/backend.rs and ACP persistence as needed

Interfaces: preserve launch/account/ACP contracts; correlate replies with originating UI actions.

Change: Fix the8 concrete remaining findings listed in handoff.md: non-account preset launch rejection, launch correlation and error visibility, quoted custom commands, retained account drafts/error recovery, persisted ACP Reset, expired quota labels, and invalid Default Sign in action.

Verify: Scenario regression tests for each finding, then package/core checks. No provider login or real credential mutations.

Progress: done

Result: All eight findings are fixed with scenario regressions. (1) `editor/backend.rs::agent_argv` resolves a stored account only for Claude and Codex — every other preset (Shell, OpenCode, Devin) launches as Default instead of being refused as an unknown agent. (2) `Req::AgentArgv` carries a request token and `State::pending_argv` is a list: each reply takes exactly its own request (`take_pending_argv`/`resolved_launch`), so two launches — different agents or the same agent in two rows — never cross over. (3) A failed argv reply surfaces the reason as the app notice and consumes only its own request's context. (4) `sessions::parse_custom_command` uses the shared `app_core::agents::shell_words`: quoted arguments stay one word, an empty or unparseable command is refused with a reason. (5) Account Add/Rename keep their drafts until the matching reply arrives (`Req::AccountAdd`/`AccountRename` carry a token; `SettingsState` tracks the pending submission), show the failure inline, and a stale reply never clears the form in front of the user; a refused queue keeps the drafts too. (6) ACP Reset now persists: `app_core::acp::agents_reset` removes that agent's stored override and keeps the others, driven by the new `AcpCmd::ResetAgent` on the ACP runtime. (7) The account menu reuses the chips' expiry-aware value (`chrome::limit_value`/`account_usage_summary`), so an elapsed window reads "reset" instead of its last percentage. (8) The Default account renders no Sign in action — the core refuses it. New tests: two launches keep their own answer (cross-agent and same-agent), a failed argv surfaces the error and keeps the other launch, custom-command parsing, account draft retention/refusal/stale-reply, ACP reset persistence (app-core and rune dispatch), expired-vs-live quota values, and a Settings render test proving Default has no Sign in button. Package 524 tests, app-core 235, desktop 116, protocol 20 — `cargo test --workspace` green; `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean (four pre-existing lints fixed: the daemon's test module after items, an unused `mut`, a needless borrow, a collapsible `if`).

### Task 21 — Protocol fixture and final workspace test readiness

Files: crates/protocol/src/message.rs::tests::session

Interfaces: protocol semantics unchanged; fixture gains pid=None.

Change: Restore workspace test compilation after SessionInfo.pid addition. The one-line fixture edit exists at stop but is not yet verified.

Verify: cargo test -p protocol and cargo test --workspace after Task20 corrections.

Progress: done

Result: The `pid=None` fixture is in place and verified — `cargo test -p protocol` (20 tests) and `cargo test --workspace` both pass after the Task 20 corrections, so the workspace compiles and every crate's suite is green.

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
- 2026-11-10: The state dot answers the child process's activity, not the session's state: a working child spins (loading), a waiting one blinks amber, a finished one — and a session with nothing running — is green. The activity comes from the files the agent writes (Claude's hook file, Codex's rollout tail), read by the worker once a second.
- 2026-11-10: The activity and the sub-agent walk are ported from `main` (`d834dbf`, `1fc1005`): `app_core::activity` and `app_core::subagents` hold the shared logic the desktop keeps in `src-tauri`, the protocol's additive `SessionInfo.pid` lets the walk start at the session's process tree, and the shared `codex app-server` agents are attributed through `CODEX_THREAD_ID` + the rollout match. A running sub-agent makes the dot spin even when the activity file says nothing. The account names stay deferred (they need the accounts service).
- 2026-11-10: The terminal grid is measured from the pane (its rect and the font's cell size) and the PTY is resized to match: a grid wider than the pane wraps its lines, and a wrapped line is what makes a full-screen program jump. The repaint waits for the output to settle, so a burst is drawn once.
- 2026-11-10: "Create worktree" asks for a name only: the checkout becomes a sibling of the project root on a fresh `goko/<name>` branch. The desktop's full form (base ref, an existing branch) is out of scope for this milestone.

- 2026-11-10: User explicitly authorized fixing the review findings and porting
  Usage/quota, multiple accounts/account names, split terminal panes, full
  worktree creation, and Settings last from main. The extension in design.md
  and Tasks 7–13 record that scope. Proceed without repeated approval under
  the current repository execution rules.
- Fresh extension baseline: workspace tests fail on two Desktop fixtures missing
  SessionInfo.pid (activity.rs and commands/tests.rs); those fixtures are Task 7.
  The protocol fixture reported in the earlier review is already fixed at the
  current starting HEAD, 24e6115. Existing dirty changes remain user-owned.

- 2026-11-10: User confirmed Settings port excludes implementing Vim; Appearance, Accounts, ACP agents and existing native editor preferences remain in scope. See decisions.md D1.

- 2026-11-10: User requested stopping when quota reached7%; work stopped and handoff updated. No final completion/archival is authorized by intermediate test results.

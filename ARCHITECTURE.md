# Goko architecture

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
  in a separate session; state is in memory only. Closing the main window or choosing Cmd-Q
  now uses the same graceful shutdown path: the backend sends the authenticated
  `system.shutdown` request, waits for the daemon lock to be released, then exits. The daemon
  stops every live PTY process group. A shutdown error or timeout keeps the app open and
  shows an error; normal quit preserves `GOKO_HOME` and `goko.db`. Force Quit and crashes do
  not guarantee cleanup.
- **The Tauri backend** talks to `gokod` over `daemon.sock` and is the only thing the UI
  talks to. It depends on the `protocol` crate and nothing else from the daemon.
- **The UI** never sees the socket. It calls Tauri commands, listens for Tauri events, and
  receives terminal bytes on a Tauri `Channel`.
- Support directory: `$GOKO_HOME`, else `~/Library/Application Support/Goko` (mode 0700),
  holding `daemon.lock`, `daemon.sock` (0600), `daemon.token` (0600), `daemon.log`,
  the app-owned `goko.db` (0600), `usage/claude/<session-id>.json` captures (`usage/claude/<account-id>/<session-id>.json` for a named account), and
  `activity/claude/<session-id>` hook files holding `working`, `waiting`, or `done`
  (files older than seven days are pruned at app startup).

## App-owned project metadata (S2)

The Tauri process opens `goko.db` in the support directory. SQLite `user_version` controls
schema migrations; schema v1 contains `projects`, `sessions`, and `settings`. This database
stores project registration and session metadata, not PTY state or terminal output. The app
continues to call `gokod` for process creation and session state; finished session rows are
removed from the app and daemon through `session.delete`.

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
  unassigned in the UI. Closing a finished session deletes this row after daemon deletion;
  if SQLite deletion fails, an idempotent retry can complete the cleanup.
- `settings` is a key/value table. `session_create` writes `last_agent` after a successful
  agent launch, but the project tree does not read it as a launch default. The D3 UI no
  longer reads or writes `active_project_id`; any value left by an earlier build is ignored.
- The sidebar is a project tree: Git projects contain the registered root checkout and
  linked worktree checkouts, listed under a collapsible **Worktrees** row; sessions group by their metadata's parent project and their
  daemon-reported `cwd`. Sessions without matching metadata appear under **Unassigned**.
  SQLite's session `cwd` remains the launch directory; checkout grouping reads the daemon's
  live `SessionInfo.cwd`.
  Project disclosure and workspace activation are separate actions. Every checkout has an
  in-memory terminal workspace with tabs and nested split panes; opening a checkout with no
  tabs starts a Shell. Selecting a session focuses its existing pane or opens it in a new tab.
  Switching checkouts keeps their current tab layouts for the lifetime of the app.
- The ProjectTree sidebar and shared Editor list/detail panel have accessible separators for
  pointer drag and keyboard adjustment. ProjectTree defaults to 320 px, is bounded to 220–480
  px, and keeps at least 320 px for the main workspace. The Editor list defaults to 288 px,
  is bounded to 180–480 px, and keeps at least 320 px for its detail pane. Each maximum
  shrinks with the available width; a narrower window keeps the minimums and scrolls
  horizontally. Widths live in UI memory across workspaces and Editor sections and reset to
  defaults when Goko restarts.
- Agent adapters (`shell`, `claude`, `codex`, `opencode`, `devin`) are discovered from `PATH` by the
  app. Launch resolves the executable again and sends an argv array through the bridge; the
  daemon remains the only PTY and child-process authority.
- On daemon restart, live sessions are gone while metadata rows remain in SQLite. The app
  hides metadata for sessions absent from `session.list`; S2 does not reattach them.

## Terminal workspaces

- A tab is one terminal pane or a nested split tree. Each pane owns one daemon session id,
  one xterm controller, one output subscription, and that session's PTY size. A daemon
  session is never rendered in multiple panes.
- Input is UTF-8 end to end: `terminal.write` base64-encodes the JS string's UTF-8 bytes and
  the daemon writes them to the PTY unchanged. macOS Simple Telex in WKWebView fires no
  composition events: each key's `input` (`insertText`, then `insertReplacementText` rewriting
  the word, e.g. `tiê` -> `tiên`) arrives before its `keyCode` 229 keydown. The controller
  captures those `input` events ahead of xterm and sends the textarea edit (backspaces for the
  changed tail, then the new tail, NFC), turns off xterm's 229 fallback, drops the IME's
  `insertText` of a key xterm already sent on keypress (space), and clears the textarea whenever
  xterm sends a key itself. The helper textarea is kept at least 1x1 px: at 0x0 Telex cannot read
  it back and falls back to marked text, losing a word's first letter. Compositions (Japanese,
  Chinese) stay with xterm. Typing `tieengs vieetj` yields `tiếng việt`.
- New Tab starts a distinct Shell in the active checkout. The project/worktree launch menu
  opens successfully created agent/custom sessions in the selected checkout. Split
  left/right and split top/bottom each start a new Shell in the same checkout. Separators
  support pointer drag and keyboard movement, and enforce a minimum size for each side.
- Tab layouts are only in memory and disappear when Goko exits. Xterm instances for open
  hidden tabs remain mounted so their scrollback and output subscriptions continue; hidden
  panes do not send PTY resize requests. Each pane in the active tab fits its assigned cell
  and sends debounced dimensions to its own session.
- The complete active tab layout fills the available terminal content area below the workspace
  controls. Opening or selecting a session focuses its tab and pane. A single pane uses the
  whole area; split trees keep all panes visible and fit each terminal to its assigned cell.
  Workspace resizing preserves split ratios. The existing pane minimum can cause scrolling
  when the available area is smaller than the combined minimum.
- Stop sends SIGTERM and leaves the exited terminal open for inspection; its manual Force
  kill action appears after five seconds. Closing a pane or tab force-kills every live session
  as soon as the daemon accepts stop for its state.
  A `created` or `starting` session stays visibly Closing until it becomes stoppable or exits
  naturally. Goko keeps the pane visible until `exited` or `failed` is confirmed, then deletes
  its daemon record, replay buffer, and SQLite metadata. Retry force-kills a still-live process.
  Force-stop/delete failures keep a visible retry action. If an older running daemon rejects
  `session.delete`, update and restart it manually before retrying; Goko never restarts a
  daemon that may own other live sessions. A missing daemon session is treated as already
  deleted so metadata cleanup can be retried.
- Removing a project does not stop its sessions. Its open tabs are rehomed under Unassigned
  and remain closable there.

## Project checkouts and Git worktrees (S3)

- A registered project remains the parent identity and its root stays the primary checkout.
  Worktrees are discovered from Git on demand; Goko stores no worktree table or checkout
  state in SQLite. Workspace state is keyed in memory by project ID plus checkout path.
- `worktree_list` exposes controls only when the registered directory is exactly the
  canonical root of a non-bare Git worktree. It runs `git worktree list --porcelain -z`
  and reports each checkout's final path component, branch or detached HEAD, canonical
  path, root/missing flags, and a `git status --porcelain=v1 -z --untracked-files=all`
  changed-file count. Missing paths remain visible with unknown dirty status until restored
  or pruned outside Goko. A normal directory or a project nested inside a repository keeps
  the flat project UI with no worktree controls.
- Worktree commands use `Command::new("git")` with separate argv values and `-C <root>`;
  no shell command string is built. `worktree_branches` lists local branches and local plus
  remote-tracking base refs without fetching. `worktree_create` validates the path, branch
  mode, and base before `git worktree add`. The path must be absent or empty and cannot
  overlap another checkout. `worktree_remove` accepts linked non-root worktrees only and
  never uses force.
- Tauri's `session_create` accepts an optional `checkout` alongside `project_id`. The app
  canonicalizes it and verifies that it is the project root or a listed worktree before
  asking the bridge to launch with that directory as `cwd`. The saved session metadata keeps
  the parent `project_id`; the sidebar uses the daemon's live `cwd` to group the session under
  the longest matching checkout path. New Shell, agent, custom-command, tab, and split
  launches inherit the active checkout. A shell changing directory moves its session row
  and checkout count after the daemon refreshes the session.
- Environment-file copying is opt-in. The preview and copy include only top-level regular
  `.env` and `.env.*` files, skip example/sample/template/dist suffixes and symlinks, never
  follow symlinks, and use exclusive creates so an existing destination is not overwritten.
  A partial failure retains the new checkout and reports copied, skipped, and failed files;
  retry repeats the copy, with prior successful files reported as skipped.
- Removal is fail-closed: the linked worktree must be clean and every daemon session across
  all projects must be exited or failed. The backend then checks session CWDs and refuses
  when an exited session is still attached to the checkout, the daemon cannot list sessions,
  or any session directory is unknown. It also refuses removal when the connected daemon is
  older than 0.1.1 because that version does not report live process CWD. The client reads
  `daemon_version` from the existing handshake; Goko does not restart an older daemon because
  it may own sessions. Goko does not kill or close sessions for removal, does not force
  removal, and preserves the branch. Confirmation warns that the checkout directory and
  local/ignored files, including copied `.env`, are deleted.
  Session creation shares the bridge's in-process operation gate with removal, and the
  daemon acquires a shared `flock` under `GOKO_HOME` for every `SessionCreate` request.
  Worktree removal takes the exclusive lock before checking sessions and running Git, so
  authenticated daemon clients also wait while a checkout is removed.
  On success, its empty in-memory workspace is dropped and the project root becomes active.
- The daemon updates the existing `SessionInfo.cwd` field as a live shell changes directory;
  its JSON shape and the daemon protocol stay unchanged. Session `list`/`get` refresh CWD
  synchronously, and the daemon watcher emits `session.updated` when a periodic refresh sees
  a change. macOS reads the process vnode path, Linux reads `/proc/<pid>/cwd`, and an
  unavailable or non-UTF-8 path is sent as an empty `cwd`. The UI groups that session under
  the project's other sessions and disables worktree removal until the session is deleted.
  It also disables removal of every checkout while any daemon session is live, even if that
  session belongs to another project, and globally if any session's CWD is unknown. Blocked
  remove actions stay focusable and expose their reason on hover or focus. `daemon_status`
  reports whether the connected daemon supports live CWD so the UI can explain when an older
  daemon needs a manual restart. Exited sessions retain their last known path.
- Every checkout workspace also has an **Editor** view, opened from the Editor
  action on a Git checkout row or the `Terminal | Editor` switch in the workspace header.
  Its sections are `Files` (a lazy tree of tracked and non-ignored untracked paths beside a
  CodeMirror editor for complete UTF-8 text files up to 1 MiB, with line numbers, lazy
  filename-based syntax support, in-editor `Cmd/Ctrl+F`, and an optional persisted Vim mode
  whose `:w` uses the existing save action; files open ready to edit, Save and `Cmd/Ctrl+S`
  write to the checkout working tree, and Markdown files — `.md`,
  `.markdown`, and `.mdx` — can switch to a rendered GFM preview. Unsaved edits prompt
  before changing files, workspaces, or the main-area view. Rendered links stay inert text
  with the href as a tooltip while images show an alt-text placeholder because the view has
  no opener or asset capability. Files also has Name and Content search modes; Content
  searches the checkout after a short debounce and opens a match at its line through the
  same unsaved-edits guard), `Changes`
  (conflict, staged, unstaged, and untracked groups with counts and per-layer diffs that
  preserve rename and deletion state), `History` (local, remote-tracking, and tag refs on a
  lane graph, 100 commits per page, selected commit files and patch), and `Stashes` (the
  repository-wide stack with the selected stash's branch hint, files, and patch). Terminal
  workspaces stay mounted under it, so tabs, splits, and running sessions survive the switch
  unchanged. The list/detail boundary can be resized; its width is shared across all four
  sections and view switches.
- The `git_view_*` Tauri commands (`changes`, `diff`, `history`, `tree`, `file`,
  `file_list`, `search`, `file_save`, `stashes`, `stash_diff`) are the Editor's Git and file boundary. Every call
  revalidates the registered project and canonical checkout path; Git calls use separate
  argv with `Command::new("git")` and `-C <checkout>`, never a shell command. The daemon
  and protocol are untouched. File and tree paths must be relative and resolve inside the
  checkout after symlink resolution. Patch and file reads cap at 1 MiB; binary, non-UTF-8,
  oversized, and missing files return an explicit kind. Saves accept only existing text
  files within that cap, compare the opened contents before replacing the file atomically,
  preserve its permissions, and update only the working tree. Content search uses
  fixed-string smart-case `git grep`, includes non-ignored untracked text files, skips
  binary files, and caps output, matches, and previews. They do not stage, commit, or
  change refs or stash state.
- Editor queries and saves are keyed by checkout context: status, diff, history, tree, and
  file read/write operations carry the project ID plus canonical checkout path, so switching checkout re-keys
  every read and stale results from another checkout can never appear. The stash list is
  keyed by project because `refs/stash` is shared across linked worktrees, and a stash
  patch is keyed by the immutable stash commit ID. Changes status/diffs and the stash list
  poll every three seconds only while their section is visible; other reads load on entry
  or selection, expose manual refresh, and keep the previous result behind a stale-data
  notice when a refresh fails.
- Fetch, commit, stage/unstage/discard, stash apply/pop/drop, checkout, merge, rebase,
  push, pull, branch deletion/renaming, lock/unlock, and automatic prune remain out of
  scope.
- Verification: `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace
  -- -D warnings`, `cargo build --workspace`, and desktop typecheck/tests/build are recorded
  with the [worktree UX change](docs/agent-devkit/changes/archive/2026-11-02-worktree-ux/tasks.md).
  Manual macOS acceptance: pending.

## Daemon lifecycle

1. `gokod` takes an exclusive `flock` on `daemon.lock`. If it is held, it prints
   `gokod: already running` and exits 0, and the first daemon is untouched.
2. It writes a fresh 32-byte random token (64 hex characters) to `daemon.token`, removes
   any stale `daemon.sock`, and binds the socket with mode 0600.
3. SIGTERM or SIGINT stops accepting, drops every connection, calls
   `SessionManager::shutdown()` (SIGTERM every session's process group, wait up to 2 s,
   then SIGKILL), removes the socket file, and exits 0.
4. The authenticated `system.shutdown` request follows the same stop and session cleanup
   path. The desktop waits for `daemon.lock` to be released before exiting; app data is kept.

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
- Requests: `system.health`, `system.version`, `system.shutdown`, `events.subscribe`, `session.create`,
  `session.list`, `session.get`, `session.stop`, `session.delete`, `terminal.write` (`data_b64`),
  `terminal.resize`, `terminal.subscribe` (`flow` opts into the output window), `terminal.ack`
  (`upto_seq`), `terminal.unsubscribe`.
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
- `SessionInfo.pid` is the PTY child's pid, set when the session becomes `running` and left `None`
  before the spawn and for a failed spawn (an older daemon omits it, which reads as `None`). The
  app uses it to find the processes under a session (see Sub-agent indicator).
- For a live session, `SessionInfo.cwd` is the process's current directory. `list` and `get`
  refresh it before returning, and a 200 ms watcher emits `session.updated` only when it
  changes. If the OS cannot provide an absolute UTF-8 path, the daemon reports an empty
  string; it preserves the last known path once the session exits.
- Exited and failed sessions stay listed until `session.delete` removes them. Deletion rejects
  live sessions, closes output subscribers, and drops the session's replay buffer; an absent
  id succeeds so app-side cleanup can safely retry.

## Agent usage indicators

The desktop backend exposes `usage_get` for the initial usage snapshot and emits
`usage://updated` when its five-second poll detects changed data. Claude launches receive a
UUIDv4 through `--session-id` and inline `--settings`; the injected statusline command saves
Claude's JSON to `GOKO_HOME/usage/claude/<session-id>.json` and forwards it to the user's
`<account folder>/settings.json` `statusLine.command`, when configured (the Default account's
folder is `$CLAUDE_CONFIG_DIR`, else `~/.claude`). Claude context fill is keyed by that session id,
while an account's limits use the newest modified capture file in its folder that reports
`rate_limits` (a newer session without them is skipped).

Accounts: a Claude or Codex session runs under the agent's active account, stored in the
`accounts` setting. The Default account launches exactly as before; a named account launches
through `/usr/bin/env CLAUDE_CONFIG_DIR=<dir>` or `/usr/bin/env CODEX_HOME=<dir>` (the daemon
has no per-session environment, so the prefix is part of the argv), captures Claude usage under
`usage/claude/<account-id>/`, and fails the launch with `invalid_argument` if its folder is
missing. The poller reads every account each tick (and at once after an account is added,
removed, or activated): `UsageSnapshot.claude` and `.codex` describe the active account and
`accounts["<agent>:<account-id>"]` holds each account's limits.

Codex limits are read from the latest `token_count` event in the last 64 KiB of the newest
modified `rollout-*.jsonl` in the two newest day directories under the account's `sessions`
folder (Default: `$CODEX_HOME/sessions`, else `~/.codex/sessions`). The UI
shows known provider limits in header chips with reset times and data age, and known Claude
context percentages in session rows. A reset limit is shown as `reset`.

## Agent activity

A running Claude or Codex session shows whether its agent is working, waiting for the user, or done.
`activity.rs` ticks every second, lists sessions through the bridge (`session_list`), and keeps only
sessions in state `running`. It sends the full map of Goko session id to `working | waiting | done` in
`activity://updated` only when it changed; `activity_get` returns the current map. If the daemon list
fails, or a source cannot be read this tick, the previous value is kept.

- Claude: the launch `--settings` also carries `hooks`. `UserPromptSubmit` and `PostToolUse` write
  `working`, `Notification` writes `waiting`, `Stop` writes `done` to
  `$GOKO_HOME/activity/claude/<claude-session-id>`; each hook command is silent and always exits 0. A
  session is recognized by `--session-id` in its argv.
- Claude runs no hook on an interrupt. `terminal_write` notes a lone Esc or input with Ctrl+C; if that
  session's delivered output contains `Interrupted` within 2 s, the interrupt time is recorded and a
  `working` hook file written before it reads as `done`. Esc that only closes a menu prints no
  `Interrupted` and changes nothing.
- Codex: a session whose argv is a lone path with stem `codex`, or `env` followed by `NAME=value` words
  and such a path (a named account), is matched to a rollout (`rollout-*.jsonl` in the two newest day
  directories under the account's `sessions` folder) with the same `cwd` as the rollout's
  `session_meta`, started no earlier than 5 s before the session. Sessions claim in start order and a
  rollout is claimed once. The last `task_started`, `task_complete`, or `turn_aborted` in the final
  64 KiB gives `working`, `done`, or `done`. Codex has no waiting event, so an approval prompt stays
  `working`, and two sessions in one directory can be mismatched when a rollout is missing.
- Shell, OpenCode, and Devin sessions have no activity and keep the plain `running` marker.

The sidebar marker for a `running` session with an activity is a spinning accent-colored ring
(`working`; a static ring when the system asks for reduced motion), a danger-toned dot (`waiting`), or a
success-toned dot (`done`); its title reads `running · <activity>`. Other states render as before.

## Sub-agent indicator

`subagents.rs` in the desktop backend polls every 2 seconds. It lists sessions through the bridge
and, if at least one running session has a `pid`, reads the process table once (`ps -axww -o
pid=,ppid=,command=`, started with an argv). For each running session it collects descendants of
its pid whose program is `claude`, `codex`, `opencode`, or `devin` (the first word's file stem, or
the word after `node` or `bun`) and publishes the map `{session id: [{agent, pid, command}]}` in
`subagents://updated` only when it changed (`subagents_get` returns the current map). A failed
listing or `ps` keeps the previous map. The sidebar shows `↳ <agent>` on a session row while
sub-agents run. The indicator is read-only: a sub-agent's output stays in its parent's pipe, and
agents running inside their parent's process are not detected.

## PTY ownership

`PtyProcess` (via `portable-pty`) opens the PTY, starts the child in a new session with
the PTY as its controlling terminal (`TERM=xterm-256color`, `COLORTERM=truecolor`), and
keeps the master. Each child gets the first inherited UTF-8 locale among `LC_ALL`, `LC_CTYPE`,
and `LANG` that the system recognizes, or `en_US.UTF-8` when none is usable. The child clears
an unavailable or non-UTF-8 `LC_ALL` and sets `LC_CTYPE` and `LANG` to the selected locale;
Goko and the daemon keep their
original environments. The child's pid is also its process-group id, so SIGTERM and SIGKILL go
to the whole group with `killpg` and take grandchildren with them. The daemon retains the
child PID to query its current working directory: macOS uses `proc_pidinfo` with
`PROC_PIDVNODEPATHINFO`, Linux reads `/proc/<pid>/cwd`, and other platforms report an
unknown directory. Commands are `argv` arrays; no shell string is ever built. A reader thread
moves output into the replay buffer and a waiter thread records the exit status. Resize
accepts 1 to 1000 in each dimension.

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

Each subscriber has a queue of 256 chunks plus one reserved slot. If a queue is full the
subscriber is removed and told `Lagged`, which the server turns into a `terminal.lagged`
event, and the client subscribes again. A client that disconnects loses its subscription and
never affects the session.

Subscribers come in two kinds. A `terminal.subscribe` without `flow` (the default, and the
only kind a daemon older than 0.1.2 understands) never holds the producer back: the reader
only ever does a non-blocking send and the PTY reader never waits for a client. With
`flow: true` the reader gates on a 256 KiB window of unacknowledged output per session: it
pauses once published output reaches the acknowledged offset plus the window (plus the one
16 KiB read in flight) and resumes when `terminal.ack` moves the offset. `upto_seq` is
an absolute end offset, not a delta; the session keeps the largest it has seen across
resubscribes and caps a future value at the published end. Replayed snapshot bytes count
against the same window, so a 1 MiB history streams in about four window round trips
instead of one burst. If no ack moves the offset for 5 s the flow subscriber is dropped
through the same `Lagged` path — a stuck client slows its own stream, never the session.
The bridge answers that drop by waiting for the next webview ack before resubscribing, so
a hung view cannot keep accumulating windows.

## Documented limit: raw replay

Replay is raw bytes, not a rendered screen. A client that subscribes (or re-subscribes)
to a full-screen program such as `vim` or `top` receives the bytes from the buffer's
start, so the screen can look wrong until the program redraws, and the buffer holds only
the last 1 MiB. This is fixed in S4 by keeping a terminal emulator snapshot in the daemon.

On macOS the tty can emit `\r\r\n` for a `\n` when its output queue backs up under CPU
load. This is the kernel's output and is harmless to a terminal, but byte-exact comparisons
against an idealized stream will see it.

## High-output behavior

PTY and daemon bridge queues are bounded, and flow subscriptions add the missing
end-to-end piece. When the desktop app connects to daemon 0.1.2 or newer it subscribes
with `flow: true`; `TerminalController` then acknowledges output as xterm parses it — one
coalesced `terminal.ack` per 64 KiB, carrying the absolute end offset of the last parsed
chunk — and the daemon's PTY reader holds unacknowledged output to the 256 KiB window
described above. A WebView that stops parsing is therefore bounded to about one window
(plus the in-flight snapshot piece and one 16 KiB read) instead of an unbounded backlog:
after 5 s without ack progress its subscription is dropped as `Lagged`, and the bridge
waits for the next ack before resubscribing. A remounted xterm starts a fresh ack count,
and an early `yes` burst now stops producing when the view stalls rather than growing its
RSS without limit.

Measured on macOS with the gate active (`pnpm -C apps/desktop tauri dev`, `gokod` 0.1.2):
during ~90 s of `yes` the WebKit WebContent process oscillated between roughly 400 and
700 MB RSS (baseline ~165 MB) as its JS heap churned — bounded, not the former
multi-GB climb — while `yes` itself stayed throttled to ~5% CPU, the producer running
at xterm's parse rate end to end. Ctrl-C killed `yes` immediately. A 512 MiB `cat`
finished in ~94 s (4.5 s of it the producer's own CPU time, the rest pacing). With the
window hidden for 30 s the producer stayed gated near 5% CPU, RSS fell to ~350 MB, and
the stream resumed with no gap on restore.

An earlier run of the manual `yes` check predates this gate: the WebView stopped responding
before Ctrl-C could be delivered, and after SIGINT stopped `yes` directly the WebKit
process still held about 4.2 GB RSS and the UI showed stale state.

## Crash behavior

Observed against the real `gokod` (see the [agent-devkit change index](docs/agent-devkit/INDEX.md)):

- **App quit before graceful shutdown was added:** Earlier direct-binary `.app` runs found
  Cmd-Q could leave `gokod` and its shell alive; a temporary launcher-wrapper run instead
  shut the daemon down. These are historical acceptance observations; current close and
  Cmd-Q behavior is described under [Daemon lifecycle](#daemon-lifecycle).
- **`kill -9` of the daemon:** the app reconnected to a fresh daemon with an empty session
  list, and the previous shell PID was absent. The first UI read already showed
  `Daemon connected`, so the disconnected banner was not captured. Reattaching to survivors
  is S4.
- **SIGTERM of the daemon:** every session process is gone before it exits.

## What S2 does not do

GUI reattach, terminal-emulator snapshots, interrupted-session reconciliation, a command
palette, settings screens, packaging, and benchmarks are later sub-projects (see [the
roadmap](docs/agent-devkit/roadmap.md)).

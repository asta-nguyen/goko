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
  linked worktree checkouts; sessions group by their metadata's parent project and their
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
- Agent adapters (`shell`, `claude`, `codex`, `opencode`) are discovered from `PATH` by the
  app. Launch resolves the executable again and sends an argv array through the bridge; the
  daemon remains the only PTY and child-process authority.
- On daemon restart, live sessions are gone while metadata rows remain in SQLite. The app
  hides metadata for sessions absent from `session.list`; S2 does not reattach them.

## Terminal workspaces

- A tab is one terminal pane or a nested split tree. Each pane owns one daemon session id,
  one xterm controller, one output subscription, and that session's PTY size. A daemon
  session is never rendered in multiple panes.
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
- Every checkout workspace also has a read-only **Editor** view, opened from the Editor
  action on a Git checkout row or the `Terminal | Editor` switch in the workspace header.
  Its sections are `Files` (a lazy tree of tracked and non-ignored untracked paths beside a
  line-numbered, read-only preview; Markdown files — `.md`, `.markdown`, and `.mdx` — open
  as a rendered GFM preview with a `Preview | Raw` toggle that keeps the mode for the app
  run, and rendered links stay inert text with the href as a tooltip while images show an
  alt-text placeholder because the view has no opener or asset capability), `Changes`
  (conflict, staged, unstaged, and untracked groups with counts and per-layer diffs that
  preserve rename and deletion state), `History` (local, remote-tracking, and tag refs on a
  lane graph, 100 commits per page, selected commit files and patch), and `Stashes` (the
  repository-wide stack with the selected stash's branch hint, files, and patch). Terminal
  workspaces stay mounted under it, so tabs, splits, and running sessions survive the switch
  unchanged. The list/detail boundary can be resized; its width is shared across all four
  sections and view switches.
- The `git_view_*` Tauri commands (`changes`, `diff`, `history`, `tree`, `file`,
  `stashes`, `stash_diff`) are the only Git read boundary. They revalidate the registered
  project and canonical checkout path on every call, reuse the separate-argv
  `Command::new("git")` rule with `-C <checkout>` and never build a shell string, and the
  daemon and protocol are untouched. File and tree paths must be relative and resolve
  inside the checkout after symlink resolution. Patch and text responses cap at 1 MiB and
  report `truncated`; binary, non-UTF-8, oversized, and missing files return an explicit
  kind instead of content. There is no file write or Git mutation anywhere in the view.
- Editor queries are keyed by checkout context: status, diff, history, tree, and file
  reads carry the project ID plus canonical checkout path, so switching checkout re-keys
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
  with the [worktree UX change](docs/agent-devkit/changes/2026-11-02-worktree-ux/tasks.md).
  Manual macOS acceptance: pending.

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
  `session.list`, `session.get`, `session.stop`, `session.delete`, `terminal.write` (`data_b64`),
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
- For a live session, `SessionInfo.cwd` is the process's current directory. `list` and `get`
  refresh it before returning, and a 200 ms watcher emits `session.updated` only when it
  changes. If the OS cannot provide an absolute UTF-8 path, the daemon reports an empty
  string; it preserves the last known path once the session exits.
- Exited and failed sessions stay listed until `session.delete` removes them. Deletion rejects
  live sessions, closes output subscribers, and drops the session's replay buffer; an absent
  id succeeds so app-side cleanup can safely retry.

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

GUI reattach, terminal-emulator snapshots, interrupted-session reconciliation, a command
palette, settings screens, packaging, and benchmarks are later sub-projects (see [the
roadmap](docs/agent-devkit/roadmap.md)).

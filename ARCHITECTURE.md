# Goko architecture

This describes what is built, taken from the code, not from the plans. The plans and
their results are in [docs/agent-devkit](docs/agent-devkit/INDEX.md).

## Process topology

```text
Rune app (apps/rune)                          gokod (crates/daemon)
┌───────────────────────────┐   Unix socket   ┌──────────────────────────────┐
│ native UI ─▶ daemon client┼────────────────▶│ server ─▶ SessionManager     │
│ (alacritty_terminal grid) │   daemon.sock   │            └─▶ PtyProcess ─▶ child
└─────────────┬─────────────┘                 └──────────────────────────────┘
              └─▶ crates/app-core ─▶ goko.db, git, language servers, ACP
```

- **`gokod`** is a separate process that owns every PTY and child process. The app starts it
  in a separate session; state is in memory only. Neither closing the main window nor Cmd-Q
  stops it: closing the window leaves the app running (`keep_running`), and quitting ends
  only the app's daemon client, so every session keeps running for the next launch to
  attach to. The daemon stops its sessions when it gets SIGTERM, SIGINT, or the
  authenticated `system.shutdown` request (see [Daemon lifecycle](#daemon-lifecycle)).
- **The Rune app** (`apps/rune`, package `goko-rune`) is the only frontend. Its daemon
  client (`apps/rune/src/daemon.rs`) runs on its own Tokio runtime thread, speaks the
  `protocol` crate to `daemon.sock`, and spawns `gokod` when the socket is absent. The UI
  thread never touches the socket.
- **`crates/app-core`** holds everything that is not UI and not PTY: the `goko.db` store,
  project and checkout validation, Git/file access, worktrees, agents/accounts/ACP,
  language servers, and usage/activity readers.
- Support directory: `$GOKO_HOME`, else `~/Library/Application Support/Goko` (mode 0700),
  holding `daemon.lock`, `daemon.sock` (0600), `daemon.token` (0600), `daemon.log`,
  the app-owned `goko.db` (0600), `usage/claude/<session-id>.json` captures, and
  `activity/claude/<session-id>` hook files holding `working`, `waiting`, or `done`
  (files older than seven days are pruned at app startup).

## App-owned project metadata

The app opens `goko.db` in the support directory. SQLite `user_version` controls schema
migrations; schema v1 contains `projects`, `sessions`, and `settings`. This database stores
project registration and session metadata, not PTY state or terminal output. The store
implementation lives in `crates/app-core`. Schema v1 is `projects(id, name, root UNIQUE,
created_at)`, `sessions(id, project_id REFERENCES projects ON DELETE SET NULL, agent, argv
JSON, cwd, title, created_at)`, and `settings(key, value)`.

- A project is a registered, canonical local directory. Adding or removing its row never
  changes files in that directory. Duplicate canonical roots return the existing project;
  removing a project sets its session metadata's `project_id` to `NULL` and does not stop
  those sessions.
- `sessions` is keyed by the daemon session UUID. The row is written best-effort after the
  daemon successfully starts a session; a storage error leaves the process running and the
  session unassigned. Closing a finished session deletes the row after daemon deletion; if
  SQLite deletion fails, an idempotent retry can complete the cleanup.
- Agent adapters (`shell`, `claude`, `codex`, `opencode`, `devin`) are discovered from
  `PATH`. Launch resolves the executable again and sends an argv array to the daemon, which
  remains the only PTY and child-process authority.
- On daemon restart, live sessions are gone while metadata rows remain in SQLite. The app
  hides metadata for sessions absent from `session.list`; it does not reattach them.
- Tab layouts and pane splits are only in memory and disappear when Goko exits. A daemon
  session is never rendered in multiple panes.
- Stop sends SIGTERM and leaves the exited terminal open for inspection; closing a pane
  force-kills the session, waits for `exited` or `failed`, then deletes its daemon record,
  replay buffer, and SQLite metadata. A failed stop/delete keeps a visible retry action. A
  missing daemon session is treated as already deleted so metadata cleanup can be retried.
  Goko never restarts a daemon that may own other live sessions.

## Project checkouts and Git worktrees

- A registered project remains the parent identity and its root stays the primary checkout.
  Worktrees are discovered from Git on demand; Goko stores no worktree table or checkout
  state in SQLite.
- `worktrees::list` exposes worktrees only when the registered directory is exactly the
  canonical root of a non-bare Git worktree. It runs `git worktree list --porcelain -z`
  and reports each checkout's final path component, branch or detached HEAD, canonical
  path, root/missing flags, and a `git status --porcelain=v1 -z --untracked-files=all`
  changed-file count. A normal directory or a project nested inside a repository has no
  worktree controls.
- Git calls use `Command::new("git")` with separate argv values and `-C <root>`; no shell
  command string is built. `branches` lists local branches and local plus remote-tracking
  base refs without fetching. `create` validates the path, branch mode, and base before
  `git worktree add`; the path must be absent or empty and cannot overlap another checkout.
  `remove` accepts linked non-root worktrees only.
- A session launched in a checkout keeps the parent `project_id`; the sidebar groups it by
  the daemon's live `cwd` under the longest matching checkout path.
- Environment-file copying is opt-in. Only top-level regular `.env` and `.env.*` files are
  copied; example/sample/template/dist suffixes and symlinks are skipped, symlinks are never
  followed, and exclusive creates mean an existing destination is not overwritten. A
  partial failure retains the new checkout and reports copied, skipped, and failed files.
- Removal is fail-closed: the linked worktree must be clean and every daemon session must
  be exited or failed with a known `cwd` outside the checkout. Removal is refused when the
  connected daemon is older than 0.1.1 (it does not report live process CWD). Goko does not
  kill sessions for removal, does not force removal, and preserves the branch.
- Session creation shares an in-process operation gate with removal, and the daemon takes a
  shared `flock` under `GOKO_HOME` for every `SessionCreate`; removal takes it exclusively.
- The daemon keeps `SessionInfo.cwd` current as a live shell changes directory. See
  [Sessions](#sessions).
- Worktree-level fetch, checkout, merge, rebase, pull, branch deletion/renaming,
  lock/unlock, and automatic prune are out of scope. The editor's Git writes are limited to
  stage/unstage/discard/commit/push; stash apply/pop/drop and fetch/pull are unimplemented.

## Shared app core

`crates/app-core` provides: the typed `BridgeError`, the `goko.db` store and migrations,
registered-project and checkout validation (`projects`), bounded Git/file reads and writes
(`git_view`, `git_write`), worktree discovery/create/remove/env-copy plus the shared Git
runner (`worktrees`), the ACP hub and agent configuration (`acp`, `agents`, `accounts`,
`subagents`), the language-server hub (`lsp`), usage and activity readers (`usage`,
`activity`), the derived Context search index (`code_index`), and `GOKO_HOME` resolution
(`home`).

- Every checkout path is revalidated against the registered project and canonicalized
  before use. File and tree paths must be relative and resolve inside the checkout after
  symlink resolution. Patch and file reads cap at 1 MiB; binary, non-UTF-8, oversized, and
  missing files return an explicit kind. Saves accept only existing text files within that
  cap, compare the opened contents before replacing the file atomically, preserve its
  permissions, and update only the working tree. Content search uses fixed-string
  smart-case `git grep`, includes non-ignored untracked text files, skips binary files, and
  caps output, matches, and previews. Context search (`code_index`) keeps a derived
  per-checkout SQLite FTS5 index under `GOKO_HOME/search/` (one file per canonical
  checkout, mode `0600`): a full pass lists the checkout with `git_view::file_list`,
  reads each file through `git_view::file`, chunks it, and records `(size, mtime_ns)`;
  a sync re-evaluates only the paths `git_view::status` names (run with
  `--no-optional-locks`, so it never takes `index.lock`), and a query opens the index
  read-only. The index is derived data; deleting it only costs a rebuild.
- Usage: Claude launches receive a UUIDv4 through `--session-id` and inline `--settings`;
  the injected statusline command saves Claude's JSON to
  `GOKO_HOME/usage/claude/<session-id>.json` and forwards it to the user's own
  `statusLine.command`. Codex limits come from the latest `token_count` event in the newest
  `rollout-*.jsonl` under `$CODEX_HOME/sessions` (default `~/.codex/sessions`).
  `usage::read_snapshot` folds both into one snapshot; `usage::spawn_poller` re-reads it
  every five seconds and emits a change.

## The Rune app

`apps/rune` is built on Rune/wgpu/winit with `rune-kit` pinned to git rev
`a7f276c3630f85a06a0230295da4d2bd172d8d3b`. Build and launch it with
`apps/rune/run-macos.sh` (see the [README](README.md)).

- **Terminal:** the daemon's PTY bytes are fed to an `alacritty_terminal` grid (`term.rs`)
  and rendered as styled text runs. Panes (`panes.rs`) are a recursive binary split layout
  per workspace tab, one terminal per pane; sessions and their lifecycle are in
  `sessions.rs`, the project/worktree tree in `sidebar.rs`.
- **Terminal input and links:** Cmd+V pastes into the focused pane (bracketed when the
  program asked, split into 192 KiB writes); rows select and copy with the pointer and
  Cmd+C (grid padding trimmed). `http(s)` URLs and `path/file.ext[:line]` tokens in a row
  are links: Cmd-click opens a URL in the browser, or a file inside the session's project
  checkout in the Editor at its line (`PendingNav::ExternalOpen`).
- **Terminal find:** Cmd+F opens a find bar over the focused pane (an overlay, so the
  PTY keeps its size). It searches the text of every row the pane's `alacritty_terminal`
  grid retains — scrollback included, wrapped rows joined — with the editor's `FindState`
  (`editor/find.rs`); `term::Haystack` maps a match back to grid cells for the highlight
  and for scrolling to it. Output that changes the grid (`Terminal::revision`) is
  searched again at most every 250 ms. Nothing is indexed and the daemon is not involved.
- **Editor:** a native workspace (Files, Changes, History, Stashes, Servers) over
  registered projects and validated checkouts, backed by `app-core` and `goko.db`.
  The changed-file lists in Changes and in a History commit detail open a context
  menu on secondary press offering `View: List | Tree` — flat rows or files nested
  under expandable directory rows — and the Changes list adds `Group By:
  None | Tracked & Untracked | Staged & Unstaged`; the Changes header's view
  button opens the same menu beside it. The List/Tree pick is one shared
  `EditorState::list_view` for both sections (the Group By pick stays on the
  Changes state), lives for the session only (no persistence, defaults `Tree`
  and `Staged & Unstaged`), survives section and checkout switches, and never
  changes row identity, filtering, or j/k navigation, which all follow the
  rendered rows.
- All Git, filesystem, storage, and highlight work runs on one worker thread behind a
  bounded queue (128 pending jobs), except Context search index passes and working-tree
  syncs, which run on a dedicated indexer thread outside that queue; replies carry a
  checkout key (`project_id` + canonical path) and a per-kind read generation, and stale
  results are dropped. Writes serialize on the same lane.
- **Context search:** the Files toolbar always shows a search box, under the file filter,
  with a `Text | Context` switch (the code names the modes `Content` and `Concept`; `Text`
  is the literal `git grep` search and the default, and the pick lasts for the session).
  `Context` ranks the checkout's text chunks from a derived SQLite FTS5 index in
  `GOKO_HOME/search/`. The aside shows the results while a search is active and the file
  tree when the file filter was edited last. A full
  pass builds it on the indexer thread (one checkout at a time, first come first served)
  and a sync before each query re-evaluates only the paths `git status` names, so a file
  added or edited a moment ago is found by the next query. The query waits at most one
  second for the sync; the first pass shows `Indexing done/total files`, and a failure
  shows `Context search unavailable`. A file reverted to `HEAD`, a branch switch, a pull,
  or a stash is not seen by a sync and is corrected by the next full pass (at most once
  per ten seconds).
- Editing uses rune-kit's native text surface with the shared 1 MiB file cap, atomic
  expected-original saves, syntax highlighting budgeted to 32 KiB / 8192 ranges,
  in-document find, and go-to-definition over the checkout's file list and content search.
- Language servers run one stdio connection per `(checkout, server)` on a dedicated runtime
  thread through `app_core::lsp::LspHub`, with debounced incremental document sync, a
  plaintext hover card, and go-to-definition requests that fall back to the heuristic path;
  diagnostics and completion are deferred.
- A dirty draft guards every navigation (file, checkout, project, section, view, window
  close, quit) with Save/Discard/Cancel; the macOS `applicationShouldTerminate:` swizzle
  vetoes OS-level quits while a draft is dirty (other platforms can only cancel window
  close — an honest gap).
- ACP commit-message generation runs on its own Tokio runtime thread with an isolated temp
  directory, denied permissions, and a timeout; the reply fills the draft only when
  unchanged.
- Stashes are read-only and repository-wide; history persists its pane layout under the
  settings key `goko.history.layout`. The code surface has an optional inline-blame column
  read through `app_core::git_view::blame`; the toggle persists under `goko.editor.blame`.
- There is no Vim mode.

## Daemon lifecycle

1. `gokod` takes an exclusive `flock` on `daemon.lock`. If it is held, it prints
   `gokod: already running` and exits 0, and the first daemon is untouched.
2. It writes a fresh 32-byte random token (64 hex characters) to `daemon.token`, removes
   any stale `daemon.sock`, and binds the socket with mode 0600.
3. SIGTERM or SIGINT stops accepting, drops every connection, calls
   `SessionManager::shutdown()` (SIGTERM every session's process group, wait up to 2 s,
   then SIGKILL), removes the socket file, and exits 0. Blocking work still running then
   gets at most 2 s more (`Runtime::shutdown_timeout`), so nothing stuck keeps it alive.
4. The authenticated `system.shutdown` request follows the same stop and session cleanup
   path. The app does not send it; it is for tools and tests. App data is kept.

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
  is not a hello gets an error and the connection is closed. So does a connection that
  sends no hello within 3 s. Nothing is served before that.
- Requests are `{"type": "...", "id": N, ...}` with a `u64` `id` that clients start at 1.
  Responses are `{"id": N, "result": ...}` or `{"id": N, "error": {code, message, hint?}}`.
  Errors that cannot be tied to a request (bad hello, bad frame) use `id` 0 and close.
- Error codes: `auth_failed`, `version_unsupported`, `frame_too_large`, `malformed`,
  `invalid_argument`, `not_found`, `spawn_failed`, `internal`.
- Requests: `system.health`, `system.version`, `system.shutdown`, `events.subscribe`, `session.create`,
  `session.list`, `session.get`, `session.stop`, `session.delete`, `terminal.write` (`data_b64`),
  `terminal.resize`, `terminal.subscribe` (`flow` opts into the output window), `terminal.ack`
  (`upto_seq`), `terminal.unsubscribe`.
- Responses are matched by `id`, not by order. A `terminal.write` is answered once its
  bytes reached the PTY: one session's writes go out and are answered in order on their
  own task, so input a child is not reading never holds up later requests, and a write
  still waiting when the child exits ends with an error.
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
  refresh it before returning, and a watcher emits `session.updated` only when it
  changes. The watcher reads the directory 200 ms after the process printed output or took
  input (a shell changes directory by running a command, which prints the next prompt) and
  every 2 s otherwise, so an idle session costs half a wakeup a second instead of five. If the OS cannot provide an absolute UTF-8 path, the daemon reports an empty
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
resubscribes and caps a future value at the published end. The reader also pauses while
a flow subscriber's queue has no room for another chunk: the window counts bytes and the
queue counts chunks, so many small reads fill the queue first, and a flow subscriber is
not dropped for that. Replayed snapshot bytes count
against the same window, so a 1 MiB history streams in about four window round trips
instead of one burst. If no ack moves the offset for 5 s the flow subscriber is dropped
through the same `Lagged` path — a stuck client slows its own stream, never the session.
The client answers that drop by waiting for the next ack before resubscribing, so a hung
view cannot keep accumulating windows. The app acknowledges every 64 KiB it has parsed,
and the rest once the output has been quiet for 30 ms, not on every frame.

The daemon also keeps each session's terminal (`terminal.rs`, an `alacritty_terminal` grid
with `protocol::SCROLLBACK_LINES` of history, the same as the app's grids) for the first
paint of a `paint: true` subscribe. It is parsed on its own thread (`gokod-pty-model`) fed
in stream order, so neither the reader nor the subscribe/ack lock waits for parsing; a
paint holds that lock and waits until the model has parsed up to its offset. The PTY
master is non-blocking: the reader and a pending write wait in `select` (macOS `poll`
does not work on PTYs), and a write gives up once the child has exited.

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
end-to-end piece. A client that subscribes with `flow: true` acknowledges output as it
parses it, and the daemon's PTY reader holds unacknowledged output to the 256 KiB window
described above. A client that stops parsing is therefore bounded to about one window
(plus the in-flight snapshot piece and one 16 KiB read) instead of an unbounded backlog:
after 5 s without ack progress its subscription is dropped as `Lagged`.

## Crash behavior

Observed against the real `gokod` (see the [agent-devkit change index](docs/agent-devkit/INDEX.md)):

- **App quit:** closing the window keeps the app running and Cmd-Q leaves the daemon and
  its sessions running (see [Process topology](#process-topology)).
- **`kill -9` of the daemon:** the app reconnected to a fresh daemon with an empty session
  list, and the previous shell PID was absent. The first UI read already showed
  `Daemon connected`, so the disconnected banner was not captured. Reattaching to survivors
  is S4.
- **SIGTERM of the daemon:** every session process is gone before it exits.

## Not built yet

Session reattach after a daemon restart, terminal-emulator snapshots, interrupted-session
reconciliation, a command palette, signed packaging, and benchmarks are later sub-projects (see [the
roadmap](docs/agent-devkit/roadmap.md)).

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versions follow the app release version (`[workspace.package]` in `Cargo.toml`).
Rename the `Unreleased` heading to the version and date when tagging a release.

End every entry with who made it and the commits, linking their profile:
``By [@login](https://github.com/login) in `abc1234`.`` Give a pull request its
number — ``Contributed by [@login](https://github.com/login) in #123
(`abc1234`, `def5678`).`` — and credit a follow-up fix by someone else in the
same entry. Keep each link on one line: What's new shows it as its text. The
`changelog` skill builds this from the commits since the last release.

## [Unreleased]

## [0.4.0] - 2026-11-20

### Added

- Pushing from the Editor now asks for the SSH key passphrase in a masked
  modal when ssh needs one; the passphrase is used for that push only, is
  never stored, and dismissing the prompt cancels the push. By
  [@asta-nguyen](https://github.com/asta-nguyen) in `aaff65a` and `bcf1975`.
- Add Project now scans a folder that is not a Git worktree root for nested
  repositories, lets you review what it found, and imports them as a group
  under a folder project or separately. A folder project shows a Files tree,
  while Changes, History and Stashes say it is not a Git worktree; child
  repositories sit under it in the sidebar and Project Map, and moving or
  removing a folder project keeps their registrations and files. By
  [@asta-nguyen](https://github.com/asta-nguyen) in `e52677b`.
- Pressing the blame note beside a line now opens the commit that last touched
  it in History, loading older commits until it is found. By
  [@asta-nguyen](https://github.com/asta-nguyen) in `958e9a8`.
- In History, a branch and its remote copy on the same commit share one pill
  (`main | origin`), pills sit before the commit message, and merge commits
  read quieter so the real commits stand out. By
  [@asta-nguyen](https://github.com/asta-nguyen) in `958e9a8`.
- The project sidebar and the Files, Changes and Stashes lists can each dock on
  the left or the right; pick the sides under Settings → Appearance → Sidebar
  position and Settings → Editor → List position. Contributed by
  [@iamdevtry](https://github.com/iamdevtry) in #34 (`e183612`, `e2a1aa7`),
  with follow-up fixes by [@asta-nguyen](https://github.com/asta-nguyen) in
  `3fcd762`.
- The code editor has an optional Vim mode, off by default and switched in
  Settings → Editor. Normal, Insert, Visual and Visual Line modes cover
  motions with counts, operators, text objects, one unnamed register, `.`
  repeat, `zz`/`zt`/`zb` scrolling, `/ ? n N *` search over the find bar, and
  a `:` bar with `:w`, `:q`, `:wq` and `:<line>`. The footer shows the mode,
  the caret is a block outside Insert, and native edits (an IME commit, a
  paste) are rejected there. In Normal mode the macOS input source must be
  ABC; Vietnamese input works in Insert. By
  [@asta-nguyen](https://github.com/asta-nguyen) in `1d03b03`.

### Changed

- Terminals run on a new engine, libghostty-vt, that keeps a terminal's
  history in about a third of the memory: one with a full 5,000-line history
  takes 5.4 MB instead of 15.8 MB, and twenty of them 149 MB instead of 353 MB
  (measured on a log-like stream at 120 columns). Output is parsed about 2.3
  times faster. A full terminal keeps about 4,800 of its 5,000 lines. Output is
  drawn on a 33 ms clock that waits a moment for a program that redraws its
  screen in several writes, so Claude Code, Codex and OpenCode are not shown
  half redrawn, and a synchronized update is held until it ends. The daemon
  uses the engine too, but it keeps running your sessions: its copies shrink
  the next time it starts. By [@asta-nguyen](https://github.com/asta-nguyen)
  in `0358e21`.

### Fixed

- Pasting an image into a terminal (a screenshot, Copy Image) did nothing. It
  now types the path of a saved PNG, which Claude Code, Codex and OpenCode
  attach as an image. Files copied in Finder paste as their paths instead of
  their names. By [@asta-nguyen](https://github.com/asta-nguyen) in `49c6056`.

## [0.3.0] - 2026-11-18

### Added

- The file lists in Changes and History are easier to read: a folder that
  holds only one folder is shown on a single line, files show just their name
  with a one-letter status (the full path is in the tooltip), each file shows
  its `+N −N` line counts and each Changes group shows its totals, and a
  row's buttons appear when you point at the row.
- Conflicts in Changes are named by what Git reports (Both modified, Deleted by
  us, Deleted by them, Added by us, …). For a conflict the inline editor cannot
  fix, such as a binary file or a file deleted on one side, the editor pane and the
  row's right-click menu offer Use Current, Use Incoming, Keep our version, Keep
  their version or Delete file for that one file.
- While a merge, rebase, cherry-pick or revert is in progress, Changes shows a
  banner with Abort (it asks first) and the Commit button reads Continue: a merge
  commits with Git's own message already filled in, the others run Git's
  `--continue`.
- Mark Resolved asks before staging a file that still contains conflict markers
  instead of refusing, and a `=======` heading underline in Markdown no longer
  counts as a conflict marker.
- Changes refreshes by itself every few seconds while the window is focused, and
  the Files tree is filled right after you open a file from Changes. An open file
  that changed on disk reloads when you have no edits in it; with edits it keeps
  them and offers Overwrite or Reload from disk.
- An Agents button in the header opens your AI agents' usage: how much of
  each quota window is spent and when it resets, for Claude Code, Codex,
  Devin, Antigravity and OpenCode Go, on macOS, Linux and Windows. Goko reads
  each agent's own sign-in on your machine and asks that agent's provider; no
  extra sign-in is needed. Press Refresh for fresh numbers. It replaces the
  usage chips in the header.
- Files you open in the Editor stay open as tabs above the editor pane, each
  keeping its own text, caret and undo history. Opening another file no
  longer interrupts a file with unsaved edits — it just opens beside it, and
  switching back finds the edits where you left them. Click a tab or press
  `⌘⌥→` / `⌘⌥←` (`Ctrl+PageDown` / `Ctrl+PageUp` off macOS) to move between
  tabs, `⌃Tab` to jump back to the one you used last, and `⌘W` or the tab's
  × to close it. When a switch away from the checkout, the section or the
  view — or closing the window or quitting — would lose work, one dialog now
  lists every file with unsaved edits and offers Save All, Discard All or
  Cancel.

## [0.2.0] - 2026-11-16

### Added

- Windows support: `gokod` and the Goko app build and run on Windows x86_64
  (10 1809 or later). The daemon serves a named pipe instead of a Unix socket,
  drives ConPTY sessions, and stops a session by killing its whole process tree
  through a Job Object. Install with
  `irm https://github.com/asta-nguyen/goko/releases/latest/download/install.ps1 | iex`.
  Stopping a session is immediate (no graceful signal), and the trash, sound,
  in-app updater, quit veto and `ps`-based agent attribution are not available
  on Windows yet.

- Tab layout survives a restart: reopening Goko brings back your tabs and how each is
  split (side by side or stacked) over the sessions still running. Split sizes start
  at 50/50 again.
- Reorder tabs: drag a tab sideways in the tab bar to move it; the other tabs step
  aside, the active tab stays active, and tabs of other projects keep their order.
- Open the file from a diff: every changed-file row in Changes, a commit's detail and a
  stash has an open icon and an Open File item in its context menu, and each file's
  header in the diff has an open button. They open the file as it is in the working tree,
  in the editor; a file deleted or renamed since then shows as missing instead of
  failing. Clicking the row still selects its diff.
- Update now: Settings → Updates installs a newer release over the copy that is
  running, so `/Applications/Goko.app` is replaced where it sits instead of a second
  copy appearing in `~/Applications`; reopen Goko afterwards. A copy it cannot replace
  (built from source, renamed, or in a folder you cannot write to) still offers the
  install command to copy. `install.sh` takes `GOKO_APP_DIR` for the folder that holds
  `Goko.app` on macOS (default `~/Applications`).
- What's new: Settings → Updates shows what changed in the release it found, taken from
  the release's changelog entry in `latest.json`.

### Changed

- Selecting text in the terminal: a plain drag selects and copies even while Claude Code,
  Codex or vim tracks the mouse, and a click with no drag still reaches the program.
  Dragging past the top or bottom of a pane scrolls through the history while you
  select, double click selects a word and triple click a line, and the copy (on release,
  or Cmd+C) holds every selected row, not just the ones on screen. Claude Code's
  fullscreen mode keeps no history for the terminal to select in, so there a plain drag
  goes to Claude Code and keeps reporting the edge while you hold past the top or
  bottom, so its own selection scrolls. Hold Option to give any program the drag
  itself; Shift always selects what is on screen.
- A session's terminal keeps 5,000 lines of history (was 10,000), in the daemon and in
  each pane alike, so a reattached pane shows the same history it had. That halves the
  memory a long session holds (about 14 MB instead of 29 MB at 120 columns), and the
  daemon hands freed memory back to the system when a session is deleted.
- The daemon parses its copy of each session's terminal on a thread of its own, so
  acknowledging, subscribing and `session.output` no longer wait for parsing, and a
  flooding session streams about 15% faster.
- The app acknowledges terminal output every 64 KiB it has parsed (and the rest once the
  output pauses), instead of on every frame.
- `gokod` reads a session's working directory after the session printed output or took
  input, and every 2 s otherwise, instead of five times a second: 20 idle sessions wake
  the daemon 11 times a second instead of 128. It also closes a connection that sends no
  hello within 3 s.
- The Context search index is about a third of its old size and builds in about half the
  time (git's own repository, 4,875 files: 110 MB and 17 s before, 34 MB and 10 s after):
  it no longer stores a second copy of every file's text. A short query such as `the` is
  about 4 times faster, because only the results shown have their text read. Existing
  indexes are rebuilt the next time Context search indexes the checkout.
- History pages load about 8 times faster on a large repository after the first one
  (git's own repository, 83,000 commits: 0.95 s a page before, 0.11 s after). A page that
  takes over 0.3 s makes Goko write the repository's commit-graph in the background, the
  cache `git gc` also writes, when it has none.
- On Linux a new install keeps its data in `$XDG_STATE_HOME/goko`
  (`~/.local/state/goko`) instead of `~/Library/Application Support/Goko`. An install
  that already has the old folder keeps using it.
- The bundled SQLite is 3.54.1 (`rusqlite` 0.42.0, was 0.40.2), which is past
  RUSTSEC-2026-0459.

### Fixed

- Selecting in Claude Code's fullscreen mode ended as soon as the drag moved: each move
  also went out as a plain hover (no button held), which told Claude Code the button was
  up. A held drag now reports only its own moves.
- A large paste into a program that was not reading its input (16 KiB or more into a
  shell running a command) froze every terminal: the daemon answered nothing else on the
  app's connection until the input was read, and never again if the program exited first.
  Writes now wait on their own per-session queue, and one still waiting when the program
  exits fails instead of hanging; `gokod` also no longer ignores SIGTERM while one waits.
- A daemon subscribe that replaced an older stream could still send a frame of the old
  stream after its response.
- A terminal whose program wrote output in many small pieces could be cut off and
  repainted while the app was behind: the daemon dropped the stream once 256 pieces were
  waiting, long before the 256 KiB it is meant to hold back at. It now pauses reading
  instead.
- While markdown streams in, an image still arriving is held back until it is complete,
  instead of showing its alt text as plain text and then replacing it.

## [0.1.3] - 2026-11-13

### Added

- Find in the terminal: `⌘F` opens a find bar over the focused pane that searches the
  pane's whole retained scrollback and screen (a match across a wrapped row counts),
  highlights every match, and steps through them with `Enter`/`⇧Enter` or Prev/Next,
  scrolling to each; `Aa` makes it case-sensitive and `Escape` closes it. Only what
  the pane still holds (10,000 rows of history) is searched.
- Context search in the Files toolbar: the search box is always visible under the
  file filter, with a `Text | Context` switch (`Text`, the literal search, is the
  default and unchanged; the pick lasts for the session). `Context` ranks the
  checkout's text chunks with a local SQLite FTS5 index (BM25), matching
  `snake_case` and `camelCase` identifiers. A file added or edited a moment ago
  is found by the next query: a working-tree sync before each query re-indexes
  only the paths `git status` names. The index is derived data under
  `GOKO_HOME/search/`. The `Search` toggle button and the separate mode row are
  gone; editing the file filter shows the file tree and editing the search box
  shows the results.
- List/Tree view and grouping for changed files: right-clicking the Changes
  file list, or pressing the view button beside the section's refresh, opens a
  menu with `View: List | Tree` (a flat list, or files nested under expandable
  directory rows) and `Group By: None | Tracked & Untracked | Staged &
  Unstaged` (a path that is both staged and unstaged shows in both groups). The
  History commit detail's changed-file list gets the same `List`/`Tree` choice
  on secondary press. One shared choice serves both sections, the session
  starts in `Tree` with `Staged & Unstaged`, and selection, patch loading,
  staging, commit, the filters and keyboard navigation are unchanged in every
  view (nothing is persisted).

## [0.1.2] - 2026-11-12

### Added

- Terminal selection while a program reports the mouse: Shift+drag selects and
  copies even when Claude Code, Codex or vim has mouse tracking on, and a
  program's OSC 52 copy from an on-screen pane goes to the system clipboard. A
  "Copied to clipboard" toast confirms a selection copy, Cmd+C, an OSC 52 copy
  and a copied breadcrumb or file path.
- Clickable URLs and `file:line` paths in terminal panes: a URL opens in the
  browser, a file in the Editor at its line.
- Keyboard: ⌥1–⌥9 pick a visible tab, ⌥←/⌥→ cycle tabs and ⌥↑/⌥↓ step through
  sidebar rows.
- A sound when a session finishes or ends while the window is unfocused: "Blow"
  on macOS, a synthesized tone elsewhere. `goko.sound_done` turns it off (on by
  default); sessions already finished at launch do not chime.
- The focused terminal's Git branch, or Detached with the short commit id,
  beside the header path; hidden outside a repository.
- An editor change gutter: thin bars beside the line numbers for the lines the
  working tree added, modified or deleted against HEAD, hidden while the draft
  is dirty and read again after the save.
- `session.output` in the daemon protocol (`gokod` 0.1.4): each session's PTY
  output offset and last output time.
- Goko's interlocking G logo in the README, macOS app bundles, and Linux
  launcher, with editable vector source and ready-to-build icon exports.
- The editor header shows the current Git branch in every section, including
  detached HEAD, with background refresh after external branch changes and
  explicit loading/error states.
- Inline merge conflict resolution with Accept Current, Incoming, or Both,
  undo, conflict navigation, and guarded per-file Mark Resolved saving/staging.

### Changed

- A session found by its process tree, without a hook integration, spins only
  while it prints output and settles to idle after 2 s of silence. With a
  `gokod` older than 0.1.4 it keeps spinning as before until the daemon is
  restarted.
- The `release` workflow builds the Linux archives (x86_64 and aarch64) beside
  macOS, so a tag publishes all three from CI.
- Every working spinner is rune-kit's activity spinner, in phase on the wall
  clock and driven by the renderer, so a working session no longer rebuilds the
  view every frame; a waiting dot blinks on a renderer-owned loop.

### Fixed

- Empty and short editor files accept clicks throughout the code pane, so
  newly created files can be edited immediately without reopening them.
- The editor's blame column stays mounted while the draft is dirty, so the code
  area no longer reflows on the first keystroke, and it formats only the caret
  line.
- A flood of terminal output is parsed in 6 ms turns instead of all at once, so
  it no longer holds up typing and painting; a keystroke's echo is drawn at
  once.

### Known issues

- `an_open_span_never_shows_its_markers` in `rune-kit` still fails under
  `cargo test --workspace` (the `markdown` feature): streamed Markdown shows
  the alt text of an image that is still being typed (`![a`) inside its
  paragraph. Fix the stream parser in `crates/rune-kit/src/markdown/stream.rs`
  or the test's expectation.
- `cargo clippy --workspace -- -D warnings` fails on macOS on two unfulfilled
  `#[expect(dead_code)]` lint expectations in `editor/quit_guard.rs`.

## [0.1.1] - 2026-11-12

### Added

- Workspaces need no name: a new workspace is created unnamed and selected,
  and is labelled after its first project ("New workspace", numbered, until
  then).
- A terminal started outside any project belongs to the workspace it was
  started in (`sessions.workspace_id`, store migration 3) and shows in that
  workspace's sidebar and tab bar while none of its projects is active.
- A shell that `cd`s into a repository joins it: the repository is registered
  in the shell's workspace and the session is renamed from its generated name.
  Project sessions never move.
- An Add Project icon beside the PROJECTS `+` in the sidebar.
- Header path: the active checkout in the Editor, the active terminal's live
  working directory otherwise, with `$HOME` shown as `~`; click to copy.
- Settings → Scroll Speed: a multiplier for mouse-wheel scrolling in terminal
  panes and views.
- Native scroll bars on the file tree, editor surfaces, history, patches and
  tables.

### Changed

- Every value pick is rune-kit's searchable Combobox, every command dropdown is
  a Menu anchored to its button (right-click menus stay at the pointer), and
  popovers are no longer capped at 320 pt. The PROJECTS `+`, a row's `+` and
  New tab share one new-session picker.
- Text that does not fit is cut in the middle by its rendered width, and its
  tooltip shows the whole text.
- The per-project tab filter applies only when the active project is in the
  selected workspace; otherwise the selected workspace's tabs show.
- Removing a workspace closes its projects' sessions and its own terminals.
- The usage chip's account menu drops from the chip itself.
- `rune-kit` is a workspace crate (`crates/rune-kit`) instead of a git
  dependency.
- The sidebar has no New Terminal button: the PROJECTS `+` is the one way to
  start a session.

### Fixed

- Stopping a session: `gokod` sends SIGHUP, then SIGTERM, and SIGKILLs after
  2 s (interactive shells ignore SIGTERM), and a dying process keeps its last
  working directory. The app force-kills a polite stop that still hangs after
  2 s, once.
- Session metadata saves upsert instead of failing on an id that is already
  known.
- Clicking a terminal tab while in the Editor now switches to the Terminal; a
  draft guard still runs first and resumes on the chosen tab.

### Known issues

- Streamed Markdown shows the alt text of an image that is still being typed
  (`![a`) inside its paragraph until the image closes and moves to its own
  block. `an_open_span_never_shows_its_markers` in `rune-kit` fails on it under
  `cargo test --workspace` (the `markdown` feature); fix the stream parser in
  `crates/rune-kit/src/markdown/stream.rs` or the test's expectation.

## [0.1.0] - 2026-11-11

### Added

- `gokod` daemon crate: owns terminal PTY processes and serves `daemon.sock`
  under `GOKO_HOME` alongside `daemon.lock`, `daemon.token`, and `daemon.log`;
  single-instance enforcement and clean SIGTERM/SIGINT shutdown.
- `protocol` crate: shared wire messages between the app and `gokod`.
- `app-core` crate: `goko.db` store (projects, agents, sessions), Git and
  worktree operations, agent/account discovery, ACP, and language-server
  plumbing shared by the app.
- `goko-rune` native app (`apps/rune`): a GPU UI built on the Rune framework
  (wgpu/winit) — no webview and no Node toolchain.
- Sidebar driven by live daemon state: project → worktrees → sessions with
  hover row actions, a new-session menu, sub-agent indicators, stop/close
  controls, and a persisted drag-resize width.
- Terminal panes on an alacritty grid fed by `terminal.subscribe` streams:
  scrollback on the wheel, IME marked text (Telex) drawn at the cursor,
  drag-dropped files and images typed into the session, resizable splits,
  and panes kept inside the window.
- Code editor on Rune's `TextEdit`: file tree, find and content search, git
  Changes (stage/unstage/discard/commit/push), History with graph lanes,
  commit detail and unified/split patch views, stashes, Markdown preview,
  quick-open, go-to-definition, hover cards and definition lookups via
  language servers, and keyboard section navigation.
- Full-file syntax highlighting for ~80 languages through `rune-highlights`.
- Indentation aids: guide lines, Tab/Shift+Tab line and block indent, Enter
  auto-indent, and a persisted space/tab unit choice.
- Inline blame column: per-line author and commit age beside the code,
  caret-line annotations, hidden while the draft is dirty, toggled from the
  editor footer (`goko.editor.blame`).
- Settings page: agent accounts with sign-in, ACP agents, usage quota chips,
  theme, and persisted UI preferences.
- Native folder dialogs for project, worktree, and account pickers, with
  failures surfaced as notice toasts.
- Terminal output backpressure (`gokod` 0.1.2): `terminal.subscribe` accepts
  `flow: true`, holding unacknowledged output to a 256 KiB window — replay
  included — and dropping a stalled subscriber after 5 s; the app
  acknowledges parsed output with `terminal.ack` every 64 KiB and waits for
  the next ack before resubscribing a stalled stream.
- Claude Code and Codex usage indicators: header chips show known account
  limits and session rows show context fill.
- Terminal state in the daemon (`gokod` 0.1.3): each session keeps its own
  terminal model, and `terminal.subscribe` accepts `paint: true` to start the
  stream from a rebuild of the scrollback, screen, cursor and modes instead of
  the last 1 MiB of raw output, so reattaching shows the real screen. The app
  uses it on a daemon that supports it and answers the program's cursor and
  device-attribute queries on that stream.
- Settings → Updates: shows the running version, checks the latest GitHub
  release at startup, every six hours and on demand, tells you when a newer one
  with a build for your machine is out, and offers the install command to copy (`GOKO_NO_UPDATE_CHECK=1` turns
  the automatic check off).
- Release packaging for macOS (Apple Silicon): pushing a tag `vX.Y.Z` builds an
  ad hoc signed `Goko.app` in CI (no Apple Developer account), publishes it with
  a `latest.json` manifest and `install.sh` as a GitHub release, and
  `curl -fsSL …/install.sh | sh` installs or updates it after checking its
  sha256 and signature (`apps/rune/package-macos.sh`, `bundle-macos.sh`). The
  installer and `package-linux.sh` also handle Linux (x86_64, aarch64), which the
  workflow does not build yet.

### Changed

- The app shell is the native Rune client (`goko-rune`); the earlier
  Tauri + React + CodeMirror/xterm.js shell is gone.

### Fixed

- The terminal wheel follows the program's mode: mouse-reporting programs get
  wheel reports (SGR or legacy, Shift scrolls history instead), programs on the
  alternate screen get Up/Down keys, and everything else scrolls the history.
  It used to do nothing in a full-screen program.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versions follow the app release version (`[workspace.package]` in `Cargo.toml`).
Rename the `Unreleased` heading to the version and date when tagging a release.

## [Unreleased]

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

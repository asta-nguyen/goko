# Terminal engine: libghostty-vt tasks

## Design

[Terminal engine: replace alacritty_terminal with libghostty-vt](design.md)

## Global Constraints

- Repo files never name the reference terminal app or its crates; run that name
  check before finishing each task.
- `libghostty-vt` comes from `vendor/ghostty` pinned at
  `291fd050b9e9c293c8adb6a140742649b7e42d3a`, built with Zig 0.16.
- `Terminal`'s public methods and `TerminalModel::{new, advance, resize,
  paint}` keep their signatures; callers outside `term.rs` change only the two
  imports in `panes.rs`.
- No protocol frame, message, version or feature change, and no `gokod`
  version bump.
- Start from a clean worktree that includes (or rebases onto) the uncommitted
  `Cargo.toml`, `crates/*/Cargo.toml`, workflow, `AGENTS.md` and
  `crates/daemon/src/pty.rs` changes; never overwrite them.
- Installing Zig 0.16 on the dev machine needs the user's go first.
- Real-app checks and measurements use a separate Goko copy with its own
  `GOKO_HOME`; never stop or restart the user's app or `gokod`.

## Impact map

```text
Entry: apps/rune/src/panes.rs::poll_terminals (feed → hold → lines per attachment); crates/daemon/src/pty.rs reader (TerminalModel::advance) and paint_at (TerminalModel::paint)
Flow: PTY bytes → pty.rs state.term.advance → subscribe: paint_at → paint ANSI + live frames (PAINT_SEQ) → apps/rune/src/daemon.rs → term.rs::Terminal::feed (vte Processor → Term) → hold/lines → render_lines → term_lines_el → rune
State changes: in-memory grids only (one per app attachment, one per daemon session); nothing persisted
External effects: query replies written to the PTY (take_replies), clipboard (take_copy, OSC 52), CI and release toolchain (Zig, submodule, libclang)
Change candidates: crates/vt/** (new), .gitmodules, vendor/ghostty, Cargo.toml, apps/rune/Cargo.toml, crates/daemon/Cargo.toml, apps/rune/src/term.rs, apps/rune/src/panes.rs (imports 15-16), crates/daemon/src/terminal.rs, crates/protocol/src/lib.rs, .github/workflows/ci.yml, .github/workflows/release.yml, AGENTS.md, README.md, ARCHITECTURE.md, CHANGELOG.md
Verification: cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace (term.rs tests 1694-2934 use only Terminal's API; daemon terminal.rs tests 339-452; crates/daemon/tests/paint.rs); CI on ubuntu-22.04, macos-15, windows-latest; real-app check per AGENTS.md
Verified at: d0d5430e89d278e063f47b358b21644583ff5b09
```

## Tasks

### Task 1 — `crates/vt`: build libghostty-vt and wrap it

```text
Files: .gitmodules; vendor/ghostty (submodule); crates/vt/Cargo.toml; crates/vt/build.rs; crates/vt/src/lib.rs::Vt; Cargo.toml (workspace member)
Interfaces: produces vt::Vt and vt::Limits (API listed in design "Interfaces") for Tasks 3-5
Change: build.rs runs `zig build` with an explicit Zig target mapped from the Rust target (x86_64/aarch64 linux-gnu, macOS, windows-msvc), links the static library (MSVC: ghostty-vt-static.lib as ghostty-vt.lib), strips the `\\?\` prefix before clang sees a path, generates bindings with bindgen, and fails with one message naming Zig 0.16, `git submodule update --init --depth 1` and libclang when one is missing. The wrapper owns the terminal and its render state, catches panics at every callback, is not Sync, and is Send only if the library allows moving it between threads. Scrollback limits are set from Limits.
Verify: `cargo test -p vt` with cases: "hi" in red → frame row 0 has "hi" with a red foreground; a 10-column wrapped line resized to 20 columns → one row; ESC[c → a DA1 reply in take_replies; OSC 52 → take_clipboard returns the text; ESC[?2026h then output → shows_change true and render_hold_deadline Some, ESC[?2026l → None; 6,000 lines with Limits { lines: 5000 } → 5,000 history rows; a byte cap smaller than the lines → fewer rows; selection of a word → selection_text. Manual: `ZIG=/nonexistent cargo build -p vt` prints the message.
Progress: done
Result: `cargo test -p vt` — 20 passed (the listed cases plus dump_vt: history/screen/cursor/modes, alternate screen, blank cursor row, cleared screen with history, background-only row, short screen); `cargo fmt -p vt -- --check` clean; `cargo clippy -p vt --all-targets -- -D warnings` clean; `ZIG=/nonexistent cargo build -p vt` prints "cannot run ... Zig 0.16 ..., the vendor/ghostty submodule ... and libclang ... are required". Findings: (1) the library drops whole pages, so the line limit keeps within a few hundred rows below it (4,773 of 5,000 at 120 columns, 3,679 at 20); (2) `dump_vt` omits the blank rows under the last text row, which shifts a replay that has scrolled, so it scrolls them back in from the bottom row (min(blank rows, history)); (3) a dump does not restore the primary screen under a running alternate-screen program and drops a background-only row's color; (4) building needs the network for Zig packages: translate-c's codeberg archive returned 504, fixed by `git fetch` of the pinned commit, `git archive` and `zig fetch` (same hash) into Zig's cache.
```

### Task 2 — Toolchain in CI, release and contributor docs

```text
Files: .github/workflows/ci.yml; .github/workflows/release.yml; AGENTS.md (Verification, Cutting a release); README.md (build from source)
Interfaces: consumes Task 1's build requirements
Change: every job checks out submodules, installs Zig 0.16 with a SHA-pinned setup action and libclang (apt clang libclang-dev on Ubuntu, LIBCLANG_PATH on Windows), and keys caches on the Zig version; runners: ubuntu-22.04, ubuntu-22.04-arm, macos-15, windows-latest (switch to windows-2022 only if windows-latest fails). Package scripts change only if the static link needs it.
Verify: CI passes `cargo test --workspace` on ubuntu-22.04, macos-15 and windows-latest; the release workflow builds all four archives through workflow_dispatch, or the user is asked before any tag is pushed.
Progress: todo
Result: edits made by Devin (swe-2-max, smart mode) and reviewed against the diff — ruby YAML parse of both workflows ok; setup-zig, submodules: true, ZIG_VERSION, libclang-dev and LIBCLANG_PATH present in both; only the four files changed; no reference-app names. Not done: the CI and release runs themselves, which need a push (cannot verify locally).
```

### Task 3 — App terminal on libghostty-vt (parity)

```text
Files: apps/rune/src/term.rs::{Terminal, Terminal::new, Terminal::feed, Terminal::resize, Terminal::scroll_by, Terminal::scroll_to_bottom, Terminal::select_start, Terminal::select_update, Terminal::select_clear, Terminal::selection_text, Terminal::haystack, Terminal::match_cells, Terminal::reveal, Terminal::cursor_cell, Terminal::wants_mouse, Terminal::alt_screen, Terminal::bracketed_paste, Terminal::mouse, render_lines, ansi_color, rgb_color}; apps/rune/src/panes.rs (imports 15-16); apps/rune/Cargo.toml; crates/protocol/src/lib.rs::SCROLLBACK_MAX_BYTES
Interfaces: consumes vt::Vt; produces term::Side and term::SelectKind for panes.rs; SCROLLBACK_MAX_BYTES (8 MB) for Task 5
Change: Terminal parses, renders and selects through Vt with the same public behavior; query replies and OSC 52 still only after a paint; key_bytes, wheel_report, mouse encoders and find_links unchanged. The hold logic stays as it is in this task.
Verify: every existing test in apps/rune/src/term.rs and apps/rune/src/view_tests.rs passes unchanged; new test `an_alacritty_paint_renders_the_same_cells`: a daemon TerminalModel paint of a styled, wrapped session with history and one of an alternate-screen session, parsed by Terminal, renders the same runs and cursor as before.
Progress: done
Result: written by Devin (swe-2-max, smart mode), reviewed and re-run by me — `cargo test -p goko-rune -p vt`: 1,048 + 27 pass (1,047 app tests before, plus `a_paint_in_the_old_daemons_shape_renders_the_same_cells`); clippy `-p goko-rune -p vt --all-targets -- -D warnings` and `cargo fmt` clean; no `alacritty` in `apps/rune`; `panes.rs` changed only in the two imports. Two existing tests asserted engine-specific facts and were adapted without dropping their point: `queries_are_answered_on_a_paint_stream_and_not_on_a_replay` (the cursor report is exact; the device-attributes reply is this engine's own `\x1b[?62;22c`, so only its `ESC [ ? … c` shape is asserted) and `a_flood_is_parsed_in_turns_and_none_of_it_is_lost` (the engine parses 2 MB inside the 6 ms turn, so it no longer asserts a second turn, only that nothing is lost). `vt` additions by Devin: `Cell.spacer`, `Vt::cursor_cell`, two tests. Known: this engine's `ESC[2J` does not push the screen into scrollback, so a paint from an OLD gokod (alacritty, which relied on that push) shows up to one screen of history fewer until the daemon restarts; a new gokod's paint is unaffected. Pacing is untouched (Task 4).
```

### Task 4 — Draw on a frame clock and reuse unchanged rows

```text
Files: apps/rune/src/term.rs::{QUIET, MAX_HOLD, Terminal::hold, Terminal::feed, Terminal::lines, render_lines, Terminal::refresh_theme}; its tests
Interfaces: consumes Vt::frame row dirty flags, shows_change and render_hold_deadline; hold() keeps returning Option<Duration>
Change: remove QUIET, MAX_HOLD and the burst; a changed, visible terminal is drawn at the next tick of a 33 ms clock, at once within TYPING of a keystroke or after a resize, and held while a synchronized update is open (at most 1 s); lines() rebuilds only dirty rows, and every row on theme change, scroll, selection change, resize and first paint.
Verify: replace `a_burst_of_output_is_drawn_once_it_settles` and `a_continuous_burst_is_held_for_at_most_max_hold` with `continuous_output_is_drawn_every_frame` (a chunk every 5 ms for 200 ms → hold() is None at least once every 33 ms), `a_synchronized_update_is_drawn_whole` (ESC[?2026h plus half a screen → lines() still shows the old screen; after ESC[?2026l → the new one) and `an_unchanged_row_is_not_rebuilt` (writing to one row of a 42-row screen rebuilds at most that row and the cursor's); `a_palette_swap_repaints_the_cached_rows` and `a_keystrokes_echo_is_not_held_back_for_the_output_to_settle` still pass.
Progress: done
Result: done by me in `apps/rune/src/term.rs` — `cargo test -p goko-rune`: 1,051 pass (`a_burst_of_output_is_drawn_once_it_settles` became `a_burst_of_output_is_drawn_on_the_next_frame_not_piece_by_piece`, `a_continuous_burst_is_held_for_at_most_max_hold` became `continuous_output_is_drawn_every_frame` (a chunk every 5 ms for 300 ms: at least 4 draws, none more than 120 ms apart), plus `a_synchronized_update_is_drawn_whole_once_it_ends`, `a_row_the_engine_did_not_change_is_moved_not_rebuilt` (the run's text buffer of an untouched row keeps its address) and `output_that_changes_nothing_on_screen_is_not_a_frame`); `a_palette_swap_repaints_the_cached_rows` and `a_keystrokes_echo_is_not_held_back_for_the_output_to_settle` pass unchanged; clippy and fmt clean. `hold()` is the wait to the next 33 ms frame and for the output to go quiet 16 ms (`QUIET`, at most `MAX_HOLD` = 100 ms after the first undrawn output) — see the Decision Log: the first version of this task dropped the quiet window and tore the UI of programs without synchronized output — or the wait to a synchronized update's end (`vt::MAX_HOLD` = 1 s), or none for the first draw, a keystroke's echo (200 ms), a resize, a scroll, a selection and a theme change. `feed` marks the grid dirty only when `vt.shows_change()`. `lines()` moves a row of the previous frame into the new one when the engine's row, its wrap flag, the theme and the cursor row are unchanged. Not measured: the felt latency in the running app (Task 6).
```

### Task 5 — Daemon terminal on libghostty-vt

```text
Files: crates/daemon/src/terminal.rs::{TerminalModel, TerminalModel::paint, write_row, Style}; its tests; crates/daemon/Cargo.toml; crates/daemon/tests/paint.rs
Interfaces: consumes vt::Vt (dump_vt or cell reads) and SCROLLBACK_MAX_BYTES
Change: TerminalModel parses through Vt; paint uses the engine's VT dump when the pinned headers have one covering scrollback, screen, cursor and modes, otherwise the current writer reads cells through Vt; the paint still stops a synchronized update first.
Verify: the daemon terminal.rs tests (`a_paint_rebuilds_history_screen_cursor_and_modes`, `a_paint_rebuilds_the_alternate_screen`, `a_sequence_split_across_chunks_is_one_sequence`, `a_resize_changes_the_grid_the_paint_describes`, `a_paint_taken_mid_synchronized_update_shows_the_frame_so_far`) compare frames through Vt and pass; crates/daemon/tests/paint.rs passes; in the separate app copy, restarting the app re-attaches with the retained history.
Progress: done
Result: written by Devin (swe-2-max, smart mode), reviewed against the file and re-run by me — `cargo test -p daemon`: 26 unit tests (the six `terminal::tests` incl. the new `a_paint_is_a_valid_start_for_a_terminal_that_already_has_content`), `paint.rs` and every integration suite that starts the real `gokod` pass; `cargo clippy -p daemon --all-targets -- -D warnings` clean; `cargo fmt -p daemon -- --check` clean; no `alacritty` left under `crates/daemon`; no reference-app names. `TerminalModel::advance` also drops the terminal's replies (the old model's listener discarded them; there is no path to the PTY). `paint()` is `ESC c` + `dump_vt()` + `ESC[?2026l` when a synchronized update is open. Not done here: the real-app re-attach check, which needs the app on the same engine (Task 6).
```

### Task 6 — Remove alacritty_terminal, docs and measurement

```text
Files: Cargo.toml; apps/rune/Cargo.toml; crates/daemon/Cargo.toml; ARCHITECTURE.md (lines 12, 145, 155, 356); docs/llm/domains/rune-chrome.md (narrative lines 823, 1020 via delta); CHANGELOG.md; this folder's delta.md
Interfaces: consumes Tasks 3-5
Change: drop the alacritty_terminal dependency; describe the engine in ARCHITECTURE.md; add a `### Changed` entry (smoother terminal output, less memory) with the changelog skill; write delta.md MODIFIED for RNC-terminal-attach and RNC-terminal-splits once daemon-terminal-state is archived, otherwise record that blocker in the Decision Log.
Verify: `rg -n alacritty --glob '!target' --glob '!CHANGELOG.md'` finds nothing outside this folder; cargo build and test pass; the real-app check from the design passes in a separate copy; the 20-terminal footprint probe on the alacritty build (HEAD before Task 3) and on this build, both numbers recorded here.
Progress: todo
Result: partly done. Done: `alacritty_terminal` is out of the root `Cargo.toml`, `apps/rune` and `crates/daemon`; `ARCHITECTURE.md` describes the engine, the frame clock and the sizing rule; whole-workspace `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass. Memory and speed (a throwaway bench in the scratchpad, release build, 20 terminals × 12,000 lines of coloured log, 120 columns, 5,000-line limit, run twice): alacritty 15.8 MB per full terminal and about 135 MB/s parsed; `vt` 5.4 MB (4,885 history rows kept) and about 290–340 MB/s; 20 terminals 353 MB against 149 MB (process footprint). These are the engine's cost, not the app's total footprint. Real-app check in a separate copy (`GokoVerify.app`, its own bundle id and `GOKO_HOME`, read through the accessibility tree because Screen Recording is not granted): colours, a wide-character row and 300 lines of scrollback attach and show; it FOUND a regression, below. Not done: the `CHANGELOG.md` entry and the wiki delta (see the Decision Log), the same check on a hand-driven session (typing, selecting, find, wheel were covered by tests, not by driving the window), and an app-level 20-terminal footprint probe.
```

## Files inspected, no change

- `crates/daemon/src/pty.rs` — `TerminalModel` users (251-313, 637); stays as is.
- `apps/rune/src/daemon.rs` — `supports_paint` (344); paint path unchanged.
- `apps/rune/src/view_tests.rs` — no alacritty types (`Side` there is `sidebar::Side`).
- `docs/agent-devkit/changes/daemon-terminal-state/delta.md` — targets `RNC-terminal-attach`.
- `docs/agent-devkit/changes/2026-11-20-memory-bounds-and-cleanup/plan.md` — no overlap with live terminals.
- vte 0.15 `src/ansi.rs` — synchronized-update buffering (36-48).
- alacritty_terminal 0.26 `src/term/mod.rs` — `damage` (458); not used after the switch.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-20: the user chose to switch to libghostty-vt outright, without a
  measurement gate first, and to follow the reference app's approach where
  useful (including its frame pacing).
- 2026-11-20: the user asked for the later tasks to be run through Devin
  (swe-2-max) with the agent managing the result. Tasks 2 and 5 ran there, and
  Task 3 too, each in `smart` permission mode because `dangerous` was refused
  by the permission system; every diff was read and every check re-run by the
  agent before the task was marked done. Task 1 and Task 4 were done by the
  agent (the engine wrapper and the pacing are core logic).
- 2026-11-20: Task 1 needs the network on the first build (Zig fetches its
  packages); codeberg.org's archive endpoint timed out, and the pinned
  `translate-c` commit was loaded into Zig's cache from a git fetch instead.
  CI's first run per runner depends on that host.
- 2026-11-20: unplanned fix found by the real-app check. A session starts at
  80×24 and the app resized its PTY only after attaching, so the first paint
  was the daemon's 80×24 terminal replayed into the pane's 134×41 grid: the
  dump flows rows (the old paint placed each row by absolute position), the
  cursor landed on the wrong row, and the shell's "erase below" on its first
  prompt wiped the 17 rows under it (`line 284…300` vanished on the first
  attach, 3 of 3 runs; the previous build did not). The reference app avoids it
  by sending its size with the attach, so the core resizes before it paints.
  Fix without a protocol change: the app sends `terminal.resize` before
  `terminal.subscribe` (`panes.rs::sync_terminals`), and `Model::paint_at`
  waits for every resize asked so far, not only for the parsed offset
  (`crates/daemon/src/pty.rs`; `gokod`'s per-connection requests are handled in
  order). Tests: `a_paint_follows_a_resize_asked_just_before_it` (fails 3 of 3
  without the wait, passes with it) and
  `a_terminal_attaches_after_the_pty_was_sized_to_its_pane`. Re-checked in the
  separate app: the 80×24-start case now shows all 41 rows (261…300). Two files
  outside the planned list changed (`pty.rs`, and `panes.rs` beyond its imports).
- 2026-11-20: regression found by the user after installing: Claude's UI
  jerked while shell was fine. Task 4 had removed the 16 ms "wait for quiet"
  window; a program that does not use synchronized output (DEC 2026) redraws
  in several writes (erase, then the rows), and a frame drawn between them is
  torn: measured 5/9, 14/23 and 21/39 torn frames at 2, 6 and 12 ms between
  writes, 0 with synchronized output. Claude Code uses synchronized output only
  when it believes the terminal supports it (its own capability check: terminal
  name in the environment, XTVERSION, DECRQM 2026 reply); sessions started
  before this build saw no answer, and Codex/OpenCode/Devin do not use it at
  all. Fix: a frame is due `FRAME` after the last draw and once the output has
  been quiet `QUIET` (16 ms), or `MAX_HOLD` (100 ms) after the first undrawn
  output, whichever comes first for the quiet part; test
  `a_redraw_split_across_writes_is_drawn_whole` (0 torn at 2, 6, 12 ms) and
  `continuous_output_is_drawn_at_least_every_max_hold`. Found while there, not
  fixed: the engine now answers DECRQM, XTVERSION, kitty keyboard and cell-size
  queries (see below), so a new Claude session enables the kitty keyboard
  protocol (`ESC[>5u`) that the app's key encoder does not speak and reads the
  cell size as 1×1 px; whether either is visible was not checked.
- 2026-11-20: Task 6's `CHANGELOG.md` entry and the wiki delta for
  `RNC-terminal-attach` / `RNC-terminal-splits` are not written yet: the
  changelog convention credits commits that do not exist until the user asks
  for them, and the open change `daemon-terminal-state` still targets
  `RNC-terminal-attach`. Both are listed as what remains.

# Terminal engine: replace alacritty_terminal with libghostty-vt

## Goal

Replace the `alacritty_terminal` emulator with `libghostty-vt` in the app and
in `gokod`, following the approach of a reference terminal app that already
ships it on macOS, Linux and Windows. The user asked for the switch outright
because the terminal feels laggy; memory is the second reason (today every
attached terminal keeps up to 5,000 lines at 24 bytes per cell, in both the app
and the daemon).

Two lag sources found in the current code are engine-independent and are fixed
in the same change, the way the reference does it:

- `Terminal::hold` (`apps/rune/src/term.rs:632-654`) holds a repaint until the
  output is quiet for 16 ms, up to 200 ms from the start of a burst, so
  continuous output is redrawn about five times a second.
- `Terminal::lines` (`apps/rune/src/term.rs:658-666`) rebuilds every visible
  row on each repaint.

## Scope

In scope:

- A new workspace crate `crates/vt`: `build.rs` builds `libghostty-vt` from the
  git submodule `vendor/ghostty`, pinned at
  `291fd050b9e9c293c8adb6a140742649b7e42d3a`, with Zig 0.16 and generates
  bindings with bindgen (libclang); `src/` holds the safe `Vt` wrapper.
- App: `apps/rune/src/term.rs` (`Terminal` internals: parse, resize, scroll,
  selection, find text, cursor, row rendering, mode reads) moves to `vt::Vt`;
  the two `alacritty_terminal` imports in `apps/rune/src/panes.rs` (`Side`,
  `SelectionType`) become types in `term.rs`.
- App pacing, as the reference does it: a visible terminal whose screen changed
  is drawn on a shared clock of about 30 frames a second; right after a
  keystroke it is drawn at once; a synchronized update (DEC 2026) is held whole
  for at most 1 s; unchanged rows keep their rendered line.
- Daemon: `crates/daemon/src/terminal.rs` (`TerminalModel::{new, advance,
  resize, paint}`) moves to `vt::Vt`.
- Protocol: `SCROLLBACK_LINES` stays; `SCROLLBACK_MAX_BYTES` is added beside it
  in `crates/protocol/src/lib.rs`, and both sides pass both limits.
- Build and release: Zig 0.16, the submodule and libclang on every CI and
  release runner, and contributor setup in `AGENTS.md` and the README.
- Removing `alacritty_terminal` from the workspace; `ARCHITECTURE.md` and
  `docs/llm` wording that names the engine; a `CHANGELOG.md` entry.

Out of scope: releasing scrollback of hidden panes in the app, kitty graphics
and image placement, the engine's key and mouse encoders (`key_bytes`,
`wheel_report` and the mouse byte encoders in `term.rs` stay; they read modes
only), `find_links` (works on row text), rune renderer changes, and any
protocol frame, message or `gokod` version change.

## Behavior

The attach flow is unchanged: the daemon's first paint (an ANSI rebuild of the
retained scrollback, screen, cursor and modes, frames marked `PAINT_SEQ`)
followed by live frames. The protocol carries ANSI bytes only, so the app can
move first while the daemon still paints from alacritty.

Must not change (existing tests: `apps/rune/src/term.rs` tests, which use only
`Terminal`'s public API, `crates/daemon/src/terminal.rs` tests,
`crates/daemon/tests/paint.rs`, `apps/rune/src/view_tests.rs`):

- styled runs, ANSI 16/256/RGB colors, inverse, wide glyphs and combining marks
  in the same cells;
- the wheel scrolls history, typing returns to the bottom, the alternate screen
  turns the wheel into arrow keys, mouse reports (X10, UTF-8, SGR, drag,
  motion) are byte-identical;
- bracketed paste; OSC 52 copy and query replies only on the live stream,
  never from a replay or paint;
- a first paint rebuilds history and screen, and a resubscribe replay that
  overlaps the parsed tail is trimmed;
- selection text, find over scrollback with matches across wrapped rows, links
  across wrapped rows, the cursor cell;
- the echo of a keystroke is drawn at once.

Changes the user sees:

- Output is drawn on a 33 ms clock. A program that does not use synchronized
  output still has its redraw (erase, then new rows, in several writes) shown
  whole: a frame waits for the output to go quiet for 16 ms, but never more
  than 100 ms after the first output not yet drawn (the old limit was 200 ms),
  so continuous output is drawn about 10 times a second instead of 5.
- A program that does use synchronized output (DEC 2026) is drawn whole at its
  end marker. The engine now answers the query for it, so a new session of such
  a program uses it.
- Scrollback is capped at 5,000 lines and `SCROLLBACK_MAX_BYTES` (8 MB) per
  terminal; a very wide or heavily styled terminal keeps fewer lines. The
  library drops whole pages, so a terminal keeps a few hundred rows fewer than
  the limit (about 4,800 at 120 columns once it has filled).
- Emoji with variation selectors and grapheme clusters follow the engine's
  width rules (mode 2027 on).
- Less memory per terminal (measured against the alacritty build in the last
  task).

## Edge cases

- **Toolchain missing.** `build.rs` fails with one message naming Zig 0.16,
  `git submodule update --init --depth 1` and libclang; it never falls back.
- **Windows.** The Zig target table covers `x86_64`/`aarch64` Linux GNU, macOS
  and Windows MSVC; on MSVC `ghostty-vt-static.lib` is linked as
  `ghostty-vt.lib`; clang is never handed a `\\?\` path.
- **Pinned revision.** The library API is not stable: the submodule moves only
  in a task that reruns all tests.
- **Mixed engines.** Until the daemon moves, the app parses an alacritty paint;
  a test checks the app renders it the same.
- **Daemon paint.** If the engine has a VT-format dump of scrollback, screen,
  cursor and modes, `paint` uses it; otherwise the current writer is ported to
  read cells through the wrapper. Either way the daemon paint tests pass.
- **Threads and FFI.** The daemon keeps the emulator under the session lock
  (`crates/daemon/src/pty.rs:251-313`); the wrapper is `Send` only if the
  library allows it, never `Sync`. No Rust panic unwinds into C, and no
  callback calls back into the same `Vt`.
- **Pacing.** Resize, theme change, scroll and selection change redraw every
  row; a synchronized update that never ends is drawn after 1 s; a hidden pane
  parses but does not draw (as today).
- **Resize and reflow.** Wrapped-line reflow keeps the current tests
  (`a_resized_grid_renders_the_new_shape`, wrapped-row find and links).
- **Open change overlap.** `RNC-terminal-attach` is still targeted by the open
  change `daemon-terminal-state` (its text is already in the wiki; the folder is
  not archived), so this change writes no delta until that one is archived;
  then a delta `MODIFIED` for `RNC-terminal-attach` and `RNC-terminal-splits`
  drops "alacritty" and adds the byte cap.
- **Uncommitted work.** `Cargo.toml`, `crates/*/Cargo.toml`,
  `.github/workflows/{ci,release}.yml`, `AGENTS.md` and
  `crates/daemon/src/pty.rs` have uncommitted changes from another session.
  Work starts from a clean worktree that includes them, or rebases onto them.

## Interfaces

- `crates/vt` (new): `Vt::new(cols, rows, Limits { lines, bytes })`,
  `write(&[u8])`, `resize(cols, rows)`, `take_replies()`, `take_clipboard()`,
  `take_title()`, `frame()` (rows of cells with colors, flags, wide and
  combining text, a per-row dirty flag, the cursor), `shows_change()`,
  `render_hold_deadline()`, `scroll_viewport(delta)`, `scroll_to_bottom()`,
  `display_offset()`, selection (`select_*`, `selection_text()`,
  `clear_selection()`), the text of any scrollback row for find, mode getters
  (mouse mode, SGR, UTF-8, motion, drag, alternate screen, bracketed paste,
  application cursor, alternate scroll), and `dump_vt()` or the cell reads the
  daemon paint needs.
- `apps/rune/src/term.rs::Terminal`: public methods keep their signatures;
  `hold()` still returns `Option<Duration>`, now the wait until the next frame
  tick, so `panes.rs::poll_terminals` is unchanged.
- `crates/daemon/src/terminal.rs::TerminalModel`: `new`, `advance`, `resize`,
  `paint` unchanged.
- `crates/protocol`: `SCROLLBACK_MAX_BYTES` added; no frame, message, version
  or feature change.
- Build: environment `ZIG` and `LIBCLANG_PATH`; submodule `vendor/ghostty`.

## Verification

- After each task: `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`; CI on `ubuntu-22.04`, `macos-15` and
  `windows-latest` with Zig installed.
- The real app per `AGENTS.md` ("Checking UI changes in the running app"),
  in a separate copy with its own `GOKO_HOME`: a shell, an editor, a full-screen
  TUI and an agent; select and copy, find, wheel, resize, split; continuous
  output stays smooth; restart and re-attach with retained history.
- Footprint: the same 20-terminal probe on the alacritty build and on the new
  build, numbers recorded in the last task.

## Related context

- [rune-chrome](../../../llm/domains/rune-chrome.md): `RNC-terminal-attach`,
  `RNC-terminal-splits`.
- [gokod-daemon](../../../llm/domains/gokod-daemon.md): daemon requirements and
  flow.

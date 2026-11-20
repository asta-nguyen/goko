# Vim mode in the native editor

Issue: 27

## Goal

Give the native code field an optional Vim mode: a pure modal engine inside
Goko, off by default, switched in Settings → Editor and stored under
`goko.editor.vim`.

Observable success: with the setting on, `h j k l`, `w b e`, `dd`, `ciw`, `p`,
`u`, `/query`, `:w` and `:q` work in the code field, the footer shows
`NORMAL`/`INSERT`/`VISUAL`/`V-LINE`, and the caret is a block outside Insert
mode; with the setting off every key behaves exactly as today, byte for byte.
The choice survives a restart.

## Scope

In:

- `apps/rune/src/editor/vim.rs` — the engine: modes, pending keys and counts,
  motions, operators, text objects, registers, `.` repeat, and the effect list.
  No Rune types, no I/O, no settings; unit-tested as a table.
- `apps/rune/src/editor/code.rs` — routing in `field_key`, applying effects to
  the shared `TextEdit`, the block caret, the mode chip, the `:` command bar,
  and the Vim-side find-bar entry points.
- `apps/rune/src/editor/state.rs` — `EditorState::vim_enabled`, the global
  unnamed register, and `CodeState.vim` (per-document mode state).
- `apps/rune/src/editor/find.rs` — "open without moving", "next match after a
  byte", and direction, used by `/ ? n N *`.
- `apps/rune/src/settings.rs` — the Settings → Editor switch and `VIM_KEY`.
- `apps/rune/src/main.rs` and `apps/rune/src/editor/backend/apply.rs` — read
  `goko.editor.vim` at startup and apply it to every mounted `CodeState`.
- `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/llm/domains/rune-editor.md` (through
  the delta), `docs/agent-devkit/INDEX.md`.

Out, with the reason:

- Terminal panes, the find bar's own query field, the commit message box and
  Servers fields: the issue's "Out of the first version"; only the code field
  routes through the engine.
- Macros, marks, named registers, Visual Block, `:s`, custom mappings and
  `.vimrc`: the issue's out-of-scope list.
- Suppressing the macOS input method in Normal mode: Rune's pinned rev
  `a7f276c` enables the IME for every focused non-secure `text_input` and has no
  per-field opt-out (`window/platform_sync.rs`), and a fork bump was declined.
  Normal mode therefore requires the ABC input source; Insert mode uses the IME
  normally. The limitation is shown in Settings and documented in the wiki.
- Any Rune, dependency or protocol change.

This change supersedes the Vim deferral recorded in the open
[rune-native-editor](../../rune-native-editor/design.md) change; that change's
files stay as history and are not edited.

## Behavior

### Setting and state

- Key `goko.editor.vim`, values `"1"` (on) and `"0"` (off); absent, unreadable,
  a failed read, or any other value means off. It is global, not per project.
- Read once at startup on the settings lane (`GetSetting`), like
  `goko.editor.indent` and `goko.editor.blame`, and applied to the active
  document and the scratch `CodeState`. Settings → Editor gains a "Vim mode"
  checkbox that writes the key through the existing `SetSetting` lane; the
  checkbox description names the Normal-mode input-source requirement.
- `CodeState.vim` holds the per-document state: `enabled`, `mode`, the pending
  key buffer, the goal column, the last change for `.`, the last `f`/`t` for
  `;`/`,`, and the last search direction. `CodeState::load` resets the mode to
  Normal and clears the pending buffer while keeping `enabled`.
- `EditorState` holds the unnamed register (`text`, `linewise`) so a yank in one
  tab pastes in another.

### Modes and routing

- Modes: Normal (the mode a document starts in), Insert, Visual (`v`), Visual
  Line (`V`).
- Routing happens at the top of `field_key`, after `preempt_key` (global
  chords) and before the existing shortcuts: keys with `cmd` or `alt` held are
  never routed to the engine, so `⌘S`, `⌘F`, `⌘G`, `⌘/`, `⌘P`, `⌘W` and the
  tab chords keep working in every mode. `Ctrl-r` is routed only in Normal.
- Insert mode passes ordinary keys to the native field (`Pass`), so IME,
  clipboard, selection, Enter auto-indent and Tab indent keep their current
  behavior. `Escape` is the exception: it leaves Insert for Normal and is
  consumed, so the field does not blur. Tab in Normal/Visual is consumed and
  does nothing.
- `Escape` in Normal cancels a pending count/operator; with nothing pending it
  is consumed and does nothing (it must not blur the field). When the find bar
  is open, the find bar's own Escape closes it first, as today.
- Outside Insert, the caret is always on a character: motions clamp to the
  first character of an empty line and to the last character of a line, never
  to the position past it. Entering Normal from Insert clamps the caret back
  onto the previous character when it sat past the line end.
- `j`/`k` remember the goal display column (tab = 4 columns) and keep it across
  shorter lines; any other motion or command clears it.

### Motions

- Character and line: `h j k l`, `0` (first character), `^` (first non-blank),
  `$` (last character), `gg` (first line), `G` (last line), counts on all.
- Words: `w b e W B E` with counts, treating letters, digits and `_` as word
  characters (Unicode alphabetic, so Vietnamese letters count) and everything
  else as punctuation.
- Find: `f t F T <char>` and `;`/`,` repeating the last one in the same or the
  opposite direction, with counts.
- Paragraphs and brackets: `{ }` over blank-line boundaries and `%` over the
  bracket pair at or after the caret on its line.
- Scroll: `zz`, `zt` and `zb` centre, top or bottom the caret line in the
  viewport without moving the caret; a count is ignored.

### Operators and text objects

- Operators `d`, `c`, `y` with any motion or text object; doubled `dd`, `cc`,
  `yy` (linewise); counts apply to both operator and motion (`2d3w`).
- Text objects: `iw aw iW aW`, `i" a"`, `i' a'`, `` i` a` ``, `i( a(`,
  `i[ a[`, `i{ a{`, `i< a<`.
- Single keys: `x` (and count), `D`, `C`, `p`, `P`, `J`, `>`, `<` (with
  motions or doubled, using the resolved indent unit), `u`, `Ctrl-r`.
- `d`, `c`, `y`, `x`, `D`, `C`, `>`, `<`, `J`, `p` and `P` are one undo step
  each: the host applies the whole effect list inside `TextEdit::undo_group`.
  `u` and `Ctrl-r` call the native undo/redo.
- `.` repeats the last Normal-mode change: an operator command with its motion,
  text object and count (replayed at the new caret), a single-key change with
  its count, or the net text inserted by the last Insert session. A Visual-mode
  change is not repeatable in v1, and neither is a `c`/`i` session that deleted
  text (only the inserted text is remembered).

### Insert mode

- `i a I A` enter Insert at the caret, after it, at the first non-blank, and at
  the line end. `o O` open a line below/above with the existing
  `indent::auto_indent` whitespace.
- Enter keeps the current `on_submit` auto-indent path; Tab keeps
  `indent::plan_indent`. Both are unchanged from today.
- Leaving Insert with Escape breaks the native typing group so the next `u`
  does not merge across modes.

### Visual modes

- `v` starts characterwise Visual with the anchor at the caret; `V` starts
  linewise Visual over the caret line. Motions extend the selection; `o` swaps
  the anchor and the caret; `Escape` returns to Normal.
- The selection is drawn as an accent highlight from the anchor through the
  character under the caret (the native caret stays collapsed on that
  character); Visual Line covers whole lines, newline included.
- `d`, `c`, `y`, `>`, `<` and `x` apply to the selection; `y` yanks, `c`
  deletes and enters Insert, `d` deletes, and Visual Line acts on whole lines.

### Registers and paste

- One unnamed register (`text`, `linewise`), written by `y`, `d`, `c`, `x`, `D`,
  `C` and Visual deletes/yanks. Every yank also writes the system clipboard
  (`Tx::copy`).
- `p` pastes after the caret (linewise: below the caret line); `P` pastes
  before it (linewise: above). The unnamed register is used when it has text;
  when it is empty, the host reads the system clipboard asynchronously through
  `Tx::read_clipboard` and applies the paste when it arrives.

### Search

- `/` opens the existing find bar without moving the caret; `?` opens it and
  records a backward direction. Typing filters as today.
- `Enter` in the bar commits: the bar closes, the caret moves to the first match
  at or after the caret the search started from (backward: at or before), and
  the match is not left selected. `Escape` closes the bar and restores the
  caret and mode.
- `n`/`N` repeat in the recorded direction (default forward), moving the caret
  to the match start without selecting it; `*` seeds the query with the word
  under the caret and jumps forward to the next match.
- `⌘F`, `⌘G` and `⇧⌘G` keep their current behavior in every mode.

### Command line

- `:` in Normal opens a one-line command bar above the code, styled like the
  find bar, with a single-line `TextEdit` and a `:` prefix. It takes focus; the
  code field blurs.
- `Enter` parses the text and closes the bar: `w`/`write` saves through the
  existing save path, `q`/`quit` closes the active tab through the draft guard,
  `wq`/`x` saves and closes the tab once the save succeeds (the existing
  `save_resume` with `PendingNav::CloseTab`), and a decimal line number reveals
  that line. Any other text leaves the bar open with an inline
  `Unknown command: :<text>` message.
- `Escape` closes the bar and returns focus to the code field.

### Block caret and mode chip

- With Vim on and the field focused outside Insert, the native caret is drawn
  transparent and a block is drawn at the caret: x from the caret line's
  display columns (tab = 4) times the mono cell advance, width one cell (a tab
  spans to its stop) and height `CODE_STYLE` line height, fill `ink.text` at
  28% alpha, inside the same scroll content as the indent guides. Insert uses
  the native bar caret.
- The footer's left side shows the mode (`NORMAL`, `INSERT`, `VISUAL`,
  `V-LINE`) and the pending keys (`2d`, `f`, `g`) while Vim is on; the chip is
  absent when Vim is off.

### Shortcuts and focus

- All existing shortcuts and chords are unchanged: `⌘S`, `⌘F`, `⌘G`/`⇧⌘G`,
  `⌘/`, `⌘P`, `⌘W`, `⌘⌥→`/`⌘⌥←`, `⌃Tab`, and `1`–`4`/`j`/`k` list keys outside
  the field.
- Read-only documents render `text(...)`, not a `text_input`, so Vim never
  applies to them.
- Switching tabs keeps each document's mode and pending state; loading a file
  resets to Normal.

### IME limitation

- Because the IME cannot be turned off for a focused field at the pinned Rune
  rev, Normal-mode letters that start a Vietnamese composition do not reach the
  engine. Any native change that still lands outside Insert — an IME commit or
  a paste — is reverted immediately, so Normal/Visual never edit the buffer.
  The Settings checkbox description says the macOS input source must be ABC in
  Normal mode; Insert mode composes and commits Vietnamese normally. This is a
  documented limitation, not a defect to fix in this change.

## Edge cases

- Empty document: every motion clamps to byte 0; `x`, `d`, `p` do nothing.
- Empty line: the caret sits at the line start; `$`, `l`, `e` stay there.
- Caret at line end in Normal after `A`+Escape: clamped onto the last
  character; an empty line keeps byte 0.
- Pending count with no motion (`3` then `Escape`): the count is dropped.
- Operator with an invalid motion (`d` then `h` at line start): nothing changes,
  the register is untouched, and the operator is cleared.
- A document edited externally (autorefresh) while a pending operator is open:
  `CodeState::text_edited` clears the pending buffer before the next key.
- Read-only or non-editable document: Vim routing is not mounted.
- Register empty and clipboard empty or unreadable: `p`/`P` change nothing.
- `:q` on a dirty document: the existing draft guard decides, exactly as the
  tab close button does; `:wq` resumes the close only after a successful save.
- `:w` on a document that fails save validation: the existing save error shows
  in the doc bar; the command bar closes.
- Find bar open: the code field is not focused, so its routing is inactive; the
  bar keeps its current Enter/Escape behavior plus the commit/cancel rules
  above.
- Setting read fails or holds an unknown value: Vim stays off.
- Mode state when the setting is switched off: the mode returns to Normal, the
  pending buffer and block caret disappear, and no key is routed.
- A key the engine does not recognize: consumed as a no-op outside Insert, so
  it never reaches the field as text.

## Interfaces

`apps/rune/src/editor/vim.rs` (new, no Rune types):

```rust
pub const VIM_KEY: &str = "goko.editor.vim";

pub enum Mode { Normal, Insert, Visual, VisualLine }
pub enum Key { Char(char), Enter, Escape, Backspace, Delete, Tab, Left, Right,
              Up, Down, Home, End, PageUp, PageDown, Ctrl(char) }

pub struct Doc<'a> {
    pub text: &'a str,
    pub focus: usize,    // caret; Visual's fixed end lives in VimState
    pub indent: IndentUnit,
}

pub struct VimState {
    pub enabled: bool,
    pub mode: Mode,
    // pending count/operator/text-object prefix, goal column, last change,
    // last f/t, last search direction
}

pub enum Effect {
    Select { anchor: usize, focus: usize },
    Replace { start: usize, end: usize, text: String },
    Undo,
    Redo,
    Yank { text: String, linewise: bool },
    Paste { before: bool, count: usize },
    Find(FindAction),          // Open { forward }, WordUnderCaret, Next, Prev
    Command(CommandAction),    // Open
    Pass,
}

pub fn step(vim: &mut VimState, doc: &Doc<'_>, key: Key) -> Vec<Effect>;
pub fn record_insert(vim: &mut VimState, inserted: &str);
```

`CodeState` gains `pub vim: vim::VimState` and
`pub vim_cmd: Option<VimCmd>` (`VimCmd { input: TextEdit, error: Option<String> }`).
`EditorState` gains `pub vim_enabled: bool` and
`pub vim_register: vim::Register` (`{ text: String, linewise: bool }`), plus
`set_vim_enabled(bool)` which mirrors the flag into the active document and the
scratch state.

`find.rs` gains `open_at(&mut self, text: &str)` (open and refresh, leaving
`active` where it was so nothing moves) and
`next_after(&mut self, from: usize, forward: bool) -> Option<Range<usize>>`
(strictly after/before `from`, wrapping, setting `active`), while
`refresh`/`next`/`prev` keep serving `⌘F`/`⌘G`. `VimState` keeps the search
origin and direction between the bar's Enter and the following `n`/`N`.

`settings.rs` adds the checkbox and `vim::VIM_KEY`; `main.rs` requests the
setting at startup; `backend/apply.rs` applies `Setting(value)` for that key.

## Verification

- `cargo test --workspace` — the `vim.rs` table covers modes, counts, every
  motion, operators, text objects, paste, `.` and clamping; `code.rs` and
  `state.rs` tests cover routing on/off, Insert pass-through, Escape consumed,
  the mode chip, the block caret element, the setting mirror and the register
  across tabs.
- `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo build --workspace`.
- Manual pass in the running app (`run-macos.sh`, per AGENTS.md): Vim off
  behaves as before; Vim on: motions, `dd`/`ciw`/`p`/`u`/`.`; block caret and
  mode chip; `/`, `?`, `n`, `N`, `*`; `:w`, `:q`, `:wq`, `:<line>`; tab switch
  keeps per-tab mode; read-only file has no Vim; and a Vietnamese IME session
  showing Insert composition works and Normal mode requires the ABC source.

## Related context

- [Rune editor](../../../../llm/domains/rune-editor.md)

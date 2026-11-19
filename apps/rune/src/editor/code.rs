//! The native code surface: a `TextEdit`-backed editor with a synchronized
//! line-number gutter, both-axis scrolling, syntax decorations, an in-file
//! find bar, read-only selectable rendering, and Cmd/Ctrl-click
//! go-to-definition through Rune's shaped-glyph `Link`s.
//!
//! Composition contract (the parent owns root state, manifests and
//! navigation):
//!
//! - [`CodeState`] holds the one persistent [`TextEdit`] the input edits
//!   through `on_edit` — undo/redo, IME, clipboard and selection stay native.
//!   Never rebuild it per frame; mutate through [`CodeState::load`] or
//!   [`CodeState::text_edited`].
//! - [`Code::build`] is the Table-style generic view: pass `get`/`get_mut`
//!   accessors into the parent's state plus optional `on_changed`/`on_save`/
//!   `on_go_to` callbacks.
//! - Syntax: the parent runs [`syntax::highlight_revision`] off-thread,
//!   stores the [`HighlightResult`] in [`CodeState::syntax`], and the view
//!   applies it only while `result.revision == code.revision`.
//! - Go-to: while `code.go_to_mod` is set (parent feeds `App::on_modifiers`),
//!   a transparent overlay mounts glyph-accurate `Link`s over the editor;
//!   clicks report a [`GoToRequest`] through `on_go_to`. The parent performs
//!   the lookup and guarded navigation. Rune has no public pointer→byte
//!   hit-test, so the overlay — not guessed coordinates — is the mechanism.

use std::ops::Range;
use std::rc::Rc;

use rune::{
    Color, El, Key, KeyEvent, LineCap, Link, LinkPaint, Live, PointerPress, ScrollPos, Shape, Span,
    TextEdit, TextHighlight, TextStyle, Tx, canvas, col, el, row, text, text_input,
};
use rune_kit::syntax::{Syntax, SyntaxTheme};
use rune_kit::{Kit, ScrollBar};

use app_core::git_view::GitViewLineChange;

use crate::editor::blame;
use crate::editor::comment;
use crate::editor::conflict::{self, Choice, Conflict};
use crate::editor::find::FindState;
use crate::editor::go_to_definition as gtd;
use crate::editor::indent::{self, IndentUnit, UnitChoice};
use crate::editor::syntax::{self, HighlightResult};
use crate::editor::vim;
use crate::theme::{self, Ink, T11M_LH, T12, T128};

/// Key of the scroll viewport: the `scroll_into_view_in` scope and the target
/// of `Tx::scroll_to_instant` reveals.
pub const SCROLL_KEY: &str = "goko-code-scroll";
/// Key of the editable field's element.
pub const FIELD_KEY: &str = "goko-code-field";
/// Key of the read-only selectable text — the `Tx::select_text` target.
pub const TEXT_KEY: &str = "goko-code-text";
/// Key of the active find match's [`TextHighlight`] — the reveal target.
pub const FIND_MATCH_KEY: &str = "goko-code-find-match";
/// Key of the Vim block caret's overlay canvas.
pub const VIM_CARET_KEY: &str = "goko-vim-caret";
/// Key of the Vim `:` command bar's input.
pub const VIM_CMD_KEY: &str = "goko-vim-cmd";

/// What a `:` command asks the host to do. `Line` is handled by the bar
/// itself (it reveals the line); the rest go to the host callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VimCommand {
    Save,
    Close,
    SaveClose,
    Line(u64),
}

/// The `:` command bar's state.
pub struct VimCmd {
    pub input: TextEdit,
    pub error: Option<String>,
}

impl VimCmd {
    pub fn new() -> VimCmd {
        VimCmd {
            input: TextEdit::new(""),
            error: None,
        }
    }
}

fn parse_vim_command(text: &str) -> Option<VimCommand> {
    match text {
        "w" | "write" => Some(VimCommand::Save),
        "q" | "quit" => Some(VimCommand::Close),
        "wq" | "x" => Some(VimCommand::SaveClose),
        other => other.parse::<u64>().ok().map(VimCommand::Line),
    }
}

/// Code metrics: 13 pt mono on a 19 pt line. The gutter shares the style so
/// its line boxes track the editor's exactly.
pub const CODE_STYLE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(19.0);
const QUERY_STYLE: TextStyle = TextStyle::new(12.0, 400.0).mono();
/// Rendered match highlights are capped so huge files stay cheap; matches
/// past the cap still navigate through `find_next`/`find_prev`.
const MAX_MATCH_HIGHLIGHTS: usize = 512;
/// Width of the inline-blame column beside the code.
const BLAME_WIDTH: f32 = 168.0;

/// A go-to-definition request: the token under the pointer or caret and its
/// document byte range. The parent resolves it (imports via
/// [`gtd::resolve_import`], names via [`gtd::pick_definition`]) and navigates
/// through its own guarded path.
#[derive(Clone, Debug)]
pub struct GoToRequest {
    pub target: gtd::GoToTarget,
    /// Document byte range of the token text (exclusive end).
    pub from: usize,
    pub to: usize,
}

/// Code-surface state owned by the parent's editor state. Generic-free: the
/// [`Code`] view reaches it through `fn` accessors.
pub struct CodeState {
    /// The editable model — the same `TextEdit` `text_input` edits through
    /// `on_edit`, so undo/redo, IME, clipboard and selection stay native.
    pub edit: TextEdit,
    /// Checkout-relative path; drives [`syntax::language_for_path`].
    pub path: String,
    /// Read-only documents render selectable text instead of an input.
    pub read_only: bool,
    /// Bumped by [`Self::load`] and [`Self::text_edited`]; syntax results apply
    /// only while `result.revision == revision`.
    pub revision: u64,
    /// Parsed once per edit; ranges always belong to the current buffer.
    pub conflicts: Vec<Conflict>,
    /// Latest worker highlight output; the view ignores stale revisions.
    pub syntax: Option<HighlightResult>,
    /// Per-line blame for the open file — who last touched each line.
    /// `None` until the first read lands.
    pub blame: Option<crate::editor::blame::Blame>,
    /// The minute the caret line's relative age was last drawn for.
    pub blame_bucket: u64,
    /// Whether the annotations are drawn for this document. The toggle is
    /// global; they additionally blank while the draft is dirty, but the
    /// column stays mounted so the code does not reflow.
    pub blame_visible: bool,
    /// Whether the inline blame column is switched on at all (mirrored from
    /// the editor's persisted setting).
    pub blame_enabled: bool,
    /// Why the last blame read failed — shown as the column's error line.
    pub blame_error: Option<String>,
    /// Lines the working tree changed against `HEAD`; empty until the first
    /// read lands.
    pub line_changes: Vec<GitViewLineChange>,
    /// Whether the change bars are drawn — they hide while the draft is dirty.
    pub changes_visible: bool,
    /// Find panel state.
    pub find: FindState,
    /// Platform go-to modifier (⌘ on macOS, Ctrl elsewhere) as last reported
    /// by `App::on_modifiers`. While set, the transparent link overlay mounts.
    pub go_to_mod: bool,
    gutter_shift: Live<[f32; 2]>,
    scroll_pos: Live<[f32; 2]>,
    scroll_size: Live<[f32; 2]>,
    gutter_text: String,
    gutter_lines: usize,
    /// Increments per [`Self::reveal`] so each request runs once.
    pub reveal_nonce: u64,
    /// Resolved indent unit for `Tab`/`Enter`/guides — see [`indent::resolve`].
    pub indent_unit: IndentUnit,
    /// Manual override mirrored from the parent; `Auto` re-detects per doc.
    pub indent_choice: UnitChoice,
    /// Modal editing state: mode, pending keys, goal column and the last
    /// `f`/`t`. `enabled` mirrors `goko.editor.vim`.
    pub vim: vim::VimState,
    /// Insert-session bookkeeping for `.`: the caret at entry and the buffer
    /// tail after it, so the net inserted text can be recovered on exit.
    pub vim_insert: Option<(usize, String)>,
    /// The Vim `/` or `?` in progress: the byte it started from and its
    /// direction. Enter commits, Escape restores.
    pub vim_search: Option<(usize, bool)>,
    /// The `:` command bar while it is open.
    pub vim_cmd: Option<VimCmd>,
}

impl CodeState {
    /// Empty state for `path`/`text`; equivalent to `load` on a fresh value.
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> CodeState {
        let mut c = CodeState {
            edit: TextEdit::new(""),
            path: String::new(),
            read_only: false,
            revision: 0,
            conflicts: Vec::new(),
            syntax: None,
            blame: None,
            blame_bucket: 0,
            blame_visible: false,
            blame_enabled: true,
            blame_error: None,
            line_changes: Vec::new(),
            changes_visible: false,
            find: FindState::new(),
            go_to_mod: false,
            gutter_shift: Live::new([0.0, 0.0]),
            scroll_pos: Live::new([0.0, 0.0]),
            scroll_size: Live::new([0.0, 0.0]),
            gutter_text: String::new(),
            gutter_lines: 0,
            reveal_nonce: 0,
            indent_unit: IndentUnit::Spaces(4),
            indent_choice: UnitChoice::Auto,
            vim: vim::VimState::default(),
            vim_insert: None,
            vim_search: None,
            vim_cmd: None,
        };
        c.load(path, text);
        c
    }

    /// Load a different document: replaces the text (fresh undo history),
    /// moves the caret to the top, bumps the revision so stale syntax drops,
    /// and refreshes find matches against the new text.
    pub fn load(&mut self, path: impl Into<String>, text: impl Into<String>) {
        self.path = path.into();
        self.edit.replace_text(text);
        self.edit.set_caret(0);
        self.vim.reset();
        self.vim_insert = None;
        self.vim_search = None;
        self.vim_cmd = None;
        self.revision += 1;
        self.conflicts = conflict::parse(&self.edit.text);
        self.find.refresh(&self.edit.text);
        self.sync_gutter();
        self.resolve_indent();
    }

    /// Re-resolve [`Self::indent_unit`] from the current text/path/choice.
    /// Runs on load and choice change only, so typing never flips the unit.
    fn resolve_indent(&mut self) {
        self.indent_unit = indent::resolve(&self.edit.text, &self.path, self.indent_choice);
    }

    /// Apply a new manual override and re-resolve the unit.
    pub fn set_indent_choice(&mut self, choice: UnitChoice) {
        self.indent_choice = choice;
        self.resolve_indent();
    }

    /// Call after any `edit` mutation that bypassed the input handlers
    /// (programmatic `insert`, marks, direct field writes): bumps the
    /// revision so stale syntax results drop, resyncs the gutter and
    /// refreshes find matches.
    pub fn text_edited(&mut self) {
        self.edit.mark_edited();
        self.vim.clear_pending();
        self.revision += 1;
        self.conflicts = conflict::parse(&self.edit.text);
        self.sync_gutter();
        self.find.refresh(&self.edit.text);
    }

    /// The scanner language for [`Self::path`], when one exists.
    pub fn language(&self) -> Option<&'static str> {
        syntax::language_for_path(&self.path)
    }

    /// 1-based line count (an empty document has one line).
    pub fn line_count(&self) -> usize {
        self.edit.text.bytes().filter(|&b| b == b'\n').count() + 1
    }

    /// The revision-matched syntax result, if the worker has answered.
    pub fn active_syntax(&self) -> Option<&HighlightResult> {
        self.syntax.as_ref().filter(|r| r.revision == self.revision)
    }

    /// Reveal `line` (1-based, clamped into the document like the desktop's
    /// `scrollIntoView`): selects the line and centres it vertically.
    pub fn reveal(&mut self, line: u32, tx: &mut Tx) {
        self.reveal_nonce += 1;
        let idx = self.clamp_line(line);
        let range = self.edit.line_range(idx);
        self.select_range(range, tx);
        if !self.read_only {
            self.edit.focused = true;
        }
        let y = self.reveal_scroll_y(idx, self.scroll_size.get()[1]);
        let x = self.scroll_pos.get()[0];
        tx.scroll_to_instant(SCROLL_KEY, x, y);
    }

    /// `line` (1-based) clamped into `[0, line_count)` as a 0-based index.
    pub fn clamp_line(&self, line: u32) -> usize {
        (line.max(1) as usize - 1).min(self.line_count().saturating_sub(1))
    }

    /// The vertical scroll offset that centres line `idx` in a `viewport_h`
    /// viewport (never negative; `scroll_to_instant` clamps the top itself).
    pub fn reveal_scroll_y(&self, idx: usize, viewport_h: f32) -> f32 {
        let line_px = CODE_STYLE.line_px();
        let top = idx as f32 * line_px;
        (top + line_px * 0.5 - viewport_h * 0.5).max(0.0)
    }

    /// Scroll the viewport so the caret is visible, leaving a line of context
    /// above and below — what a motion or an Insert session needs after the
    /// caret moved programmatically.
    pub fn scroll_to_caret(&self, tx: &mut Tx) {
        let caret = self.edit.focus.min(self.edit.text.len());
        let idx = self.edit.line_of_byte(caret);
        let line_px = CODE_STYLE.line_px();
        let pos = self.scroll_pos.get();
        let size = self.scroll_size.get();
        if size[1] <= 0.0 {
            return;
        }
        let top = idx as f32 * line_px;
        let bottom = top + line_px;
        let (mut x, mut y) = (pos[0], pos[1]);
        if top < y {
            y = (top - line_px).max(0.0);
        } else if bottom > y + size[1] {
            y = bottom - size[1] + line_px;
        }
        let cell_w = rune::text::advance("0", CODE_STYLE).max(1.0);
        if size[0] > 0.0 {
            let line_start = self.edit.line_range(idx).start;
            let caret_x = display_cols(&self.edit.text[line_start..caret]) as f32 * cell_w;
            if caret_x < x {
                x = (caret_x - cell_w).max(0.0);
            } else if caret_x + cell_w > x + size[0] {
                x = caret_x + cell_w - size[0];
            }
        }
        if (x - pos[0]).abs() > 0.5 || (y - pos[1]).abs() > 0.5 {
            tx.scroll_to_instant(SCROLL_KEY, x, y);
        }
    }

    /// Open the find bar, seeding the query from a single-line selection.
    pub fn open_find(&mut self, tx: &mut Tx) {
        self.vim_search = None;
        let (a, b) = self.edit.range();
        if a != b {
            let sel = &self.edit.text[a..b];
            if !sel.contains('\n') {
                self.find.set_query(sel);
            }
        }
        self.find.open_panel();
        self.find.refresh(&self.edit.text);
        self.edit.focused = false;
        self.find.query.focused = true;
        if let Some(r) = self.find.active_range() {
            self.select_match(&r, tx);
        }
    }

    /// Hide the find bar and return focus to the editor.
    pub fn close_find(&mut self, tx: &mut Tx) {
        self.vim_search = None;
        self.find.close();
        self.find.query.focused = false;
        if !self.read_only {
            self.edit.focused = true;
        }
        let _ = tx;
    }

    /// Vim `/` or `?`: open the bar on a fresh query without moving the caret.
    pub fn open_find_vim(&mut self, forward: bool, _tx: &mut Tx) {
        self.find.set_query("");
        self.find.open_at(&self.edit.text);
        self.vim_search = Some((self.edit.focus, forward));
        self.edit.focused = false;
        self.find.query.focused = true;
    }

    /// Enter in a Vim search: close the bar and land on the next match.
    pub fn commit_vim_search(&mut self, tx: &mut Tx) {
        let Some((origin, forward)) = self.vim_search.take() else {
            return;
        };
        let range = self.find.next_after(origin, forward);
        self.close_find(tx);
        if let Some(r) = range {
            self.edit.set_caret(r.start);
            self.scroll_to_caret(tx);
        }
    }

    /// Escape in a Vim search: close the bar and restore the caret.
    pub fn cancel_vim_search(&mut self, tx: &mut Tx) {
        let Some((origin, _)) = self.vim_search.take() else {
            return;
        };
        self.close_find(tx);
        let at = origin.min(self.edit.text.len());
        self.edit.set_caret(at);
        self.scroll_to_caret(tx);
    }

    /// `n`/`N`: step the last search and put the caret on the match start.
    pub fn find_step_vim(&mut self, forward: bool, tx: &mut Tx) -> Option<Range<usize>> {
        let range = if forward {
            self.find.next()
        } else {
            self.find.prev()
        }?;
        self.edit.set_caret(range.start);
        self.scroll_to_caret(tx);
        Some(range)
    }

    /// `*`: search the word under the caret and land on the next match.
    pub fn search_word_vim(&mut self, tx: &mut Tx) {
        let Some(word) = vim::word_under_caret(&self.edit.text, self.edit.focus) else {
            return;
        };
        self.find.set_query(&word);
        self.find.refresh(&self.edit.text);
        self.vim_search = None;
        if let Some(range) = self.find.next_after(self.edit.focus, true) {
            self.edit.set_caret(range.start);
            self.scroll_to_caret(tx);
        }
    }

    /// `:`: open the command bar on a fresh input and take focus.
    pub fn open_cmd_vim(&mut self) {
        let mut cmd = VimCmd::new();
        cmd.input.focused = true;
        self.vim_cmd = Some(cmd);
        self.edit.focused = false;
    }

    /// Escape in the command bar: close it and return focus to the code.
    pub fn close_cmd_vim(&mut self) {
        self.vim_cmd = None;
        if !self.read_only {
            self.edit.focused = true;
        }
    }

    /// Enter in the command bar: parse and close, or keep it open with the
    /// unknown-command message. Returns the host action, if any.
    pub fn submit_cmd_vim(&mut self, tx: &mut Tx) -> Option<VimCommand> {
        let text = self.vim_cmd.as_ref()?.input.text.trim().to_string();
        match parse_vim_command(&text) {
            Some(VimCommand::Line(line)) => {
                self.vim_cmd = None;
                self.edit.focused = true;
                self.reveal(line as u32, tx);
                None
            }
            Some(action) => {
                self.vim_cmd = None;
                self.edit.focused = true;
                Some(action)
            }
            None => {
                if let Some(cmd) = self.vim_cmd.as_mut() {
                    cmd.error = Some(format!("Unknown command: :{text}"));
                }
                None
            }
        }
    }

    /// Advance to the next find match (wrapping), selecting and revealing it.
    /// Works with the panel closed so ⌘G can navigate silently.
    pub fn find_next(&mut self, tx: &mut Tx) -> Option<Range<usize>> {
        let r = self.find.next()?;
        self.select_match(&r, tx);
        Some(r)
    }

    /// Go to the previous find match (wrapping), selecting and revealing it.
    pub fn find_prev(&mut self, tx: &mut Tx) -> Option<Range<usize>> {
        let r = self.find.prev()?;
        self.select_match(&r, tx);
        Some(r)
    }

    /// `zz`/`zt`/`zb`: put the caret line in the middle, at the top or at the
    /// bottom of the viewport without moving the caret.
    pub fn scroll_line(&self, action: vim::Scroll, tx: &mut Tx) {
        let idx = self
            .edit
            .line_of_byte(self.edit.focus.min(self.edit.text.len()));
        let line_px = CODE_STYLE.line_px();
        let pos = self.scroll_pos.get();
        let viewport = self.scroll_size.get()[1];
        if viewport <= 0.0 {
            return;
        }
        let top = idx as f32 * line_px;
        let y = match action {
            vim::Scroll::Center => top + line_px * 0.5 - viewport * 0.5,
            vim::Scroll::Top => top,
            vim::Scroll::Bottom => top + line_px - viewport,
        }
        .max(0.0);
        tx.scroll_to_instant(SCROLL_KEY, pos[0], y);
    }

    /// The Visual selection's byte range for the view's highlight, or `None`
    /// outside Visual.
    pub fn vim_visual_range(&self) -> Option<Range<usize>> {
        if !self.vim.enabled || self.read_only {
            return None;
        }
        self.vim.visual_range(&self.edit.text, self.edit.focus)
    }

    /// The go-to token containing document byte `byte`, if any. Classifies
    /// exactly like the pointer overlay: names inside comments/strings are
    /// not targets; import targets bypass trivia, so commented-out imports
    /// still resolve.
    pub fn go_to_token_at(&self, byte: usize) -> Option<GoToRequest> {
        let byte = byte.min(self.edit.text.len());
        let line_idx = self.edit.line_of_byte(byte);
        let lr = self.edit.line_range(line_idx);
        let line = &self.edit.text[lr.clone()];
        let tok = gtd::token_at(line, byte - lr.start)?;
        let from = lr.start + tok.from;
        let to = lr.start + tok.to;
        let trivia = self.trivia();
        if token_blocked(&trivia, is_import(&tok.target), from, to) {
            return None;
        }
        Some(GoToRequest {
            target: tok.target,
            from,
            to,
        })
    }

    /// Document byte ranges of every go-to token, filtered by the
    /// whole-document lexical trivia scan — never capped, so a target near
    /// the end of a large file still links. The overlay maps these to
    /// [`Link`]s.
    pub fn go_to_ranges(&self) -> Vec<Range<usize>> {
        let trivia = self.trivia();
        let mut out = Vec::new();
        let mut offset = 0usize;
        for line in self.edit.text.split_inclusive('\n') {
            let line_text = line.strip_suffix('\n').unwrap_or(line);
            for tok in gtd::tokens_in_line(line_text) {
                let from = offset + tok.from;
                let to = offset + tok.to;
                if to <= from || token_blocked(&trivia, is_import(&tok.target), from, to) {
                    continue;
                }
                out.push(from..to);
            }
            offset += line.len();
        }
        out
    }

    /// Comment/string byte ranges of the whole document, from the lexical
    /// classifier — independent of the highlight result, so exclusion works
    /// document-wide and while a highlight job is pending or stale.
    fn trivia(&self) -> Vec<(Range<usize>, syntax::Trivia)> {
        syntax::trivia_ranges(&self.edit.text, self.language())
    }

    /// Select `range` in whichever surface is live and scroll the keyed find
    /// highlight into view.
    fn select_match(&mut self, range: &Range<usize>, tx: &mut Tx) {
        self.select_range(range.clone(), tx);
        tx.scroll_into_view_in(SCROLL_KEY, FIND_MATCH_KEY);
    }

    fn select_range(&mut self, range: Range<usize>, tx: &mut Tx) {
        if self.read_only {
            tx.select_text(TEXT_KEY, range);
        } else {
            self.edit.set_selection(range.start, range.end);
        }
    }

    fn sync_gutter(&mut self) {
        let n = self.line_count();
        if n == self.gutter_lines {
            return;
        }
        self.gutter_lines = n;
        let w = n.to_string().len();
        let mut s = String::with_capacity(n * (w + 1));
        for i in 1..=n {
            if i > 1 {
                s.push('\n');
            }
            let num = i.to_string();
            for _ in num.len()..w {
                s.push(' ');
            }
            s.push_str(&num);
        }
        self.gutter_text = s;
    }

    pub fn gutter_text(&self) -> &str {
        &self.gutter_text
    }

    pub fn gutter_width(&self) -> f32 {
        let digits = self.gutter_lines.max(1).to_string().len();
        20.0 + digits as f32 * 8.0
    }
}

/// Parent-state callback shared by the field's change/save hooks.
type PressCallback<S> = Rc<dyn Fn(&mut S, PointerPress, &mut Tx)>;
type StateCallback<S> = Rc<dyn Fn(&mut S, &mut Tx)>;
/// Go-to callback: receives the clicked [`GoToRequest`].
type GoToCallback<S> = Rc<dyn Fn(&mut S, GoToRequest, &mut Tx)>;
/// Indent-override callback: receives the next [`UnitChoice`] to persist.
type IndentCallback<S> = Rc<dyn Fn(&mut S, UnitChoice, &mut Tx)>;
/// Blame-toggle callback: receives the new enabled state.
type BlameCallback<S> = Rc<dyn Fn(&mut S, bool, &mut Tx)>;
/// Blame-annotation callback: receives the oid of the commit to open.
type BlameOpenCallback<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;
/// First-refusal key callback: `true` consumes the key before the field's
/// own handling sees it.
type KeyCallback<S> = Rc<dyn Fn(&mut S, &KeyEvent, &mut Tx) -> bool>;
type VimCommandCallback<S> = Rc<dyn Fn(&mut S, VimCommand, &mut Tx)>;

/// The code surface view. `get`/`get_mut` reach the parent's [`CodeState`];
/// callbacks are optional and generic over the app state `S`.
pub struct Code<S> {
    get: fn(&S) -> &CodeState,
    get_mut: fn(&mut S) -> &mut CodeState,
    register: Option<fn(&mut S) -> &mut vim::Register>,
    on_changed: Option<StateCallback<S>>,
    on_save: Option<StateCallback<S>>,
    on_go_to: Option<GoToCallback<S>>,
    on_indent: Option<IndentCallback<S>>,
    on_blame: Option<BlameCallback<S>>,
    on_blame_open: Option<BlameOpenCallback<S>>,
    on_send: Option<StateCallback<S>>,
    on_secondary: Option<PressCallback<S>>,
    on_vim_command: Option<VimCommandCallback<S>>,
    preempt_key: Option<KeyCallback<S>>,
}

impl<S: 'static> Code<S> {
    pub fn new(get: fn(&S) -> &CodeState, get_mut: fn(&mut S) -> &mut CodeState) -> Code<S> {
        Code {
            get,
            get_mut,
            register: None,
            on_changed: None,
            on_save: None,
            on_go_to: None,
            on_indent: None,
            on_blame: None,
            on_blame_open: None,
            on_send: None,
            on_secondary: None,
            on_vim_command: None,
            preempt_key: None,
        }
    }

    /// Called after every accepted text change (typing, paste, undo, IME
    /// commit) — the place to mark drafts dirty or schedule a save.
    pub fn on_changed(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Code<S> {
        self.on_changed = Some(Rc::new(f));
        self
    }

    /// Called on ⌘S / Ctrl+S while the editor is focused.
    pub fn on_save(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Code<S> {
        self.on_save = Some(Rc::new(f));
        self
    }

    /// Called when a go-to link is clicked (platform modifier already held).
    /// The parent resolves [`GoToRequest::target`] and navigates.
    pub fn on_go_to(mut self, f: impl Fn(&mut S, GoToRequest, &mut Tx) + 'static) -> Code<S> {
        self.on_go_to = Some(Rc::new(f));
        self
    }

    /// Called when the indent chip is pressed; receives the next
    /// [`UnitChoice`] in the Auto → 2 → 4 → 8 → Tab cycle.
    pub fn on_indent(mut self, f: impl Fn(&mut S, UnitChoice, &mut Tx) + 'static) -> Code<S> {
        self.on_indent = Some(Rc::new(f));
        self
    }

    /// Called when the blame chip is pressed; receives the new enabled state.
    pub fn on_blame(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Code<S> {
        self.on_blame = Some(Rc::new(f));
        self
    }

    /// Called when the caret line's blame annotation is pressed; receives the
    /// oid of the commit that last touched the line.
    pub fn on_blame_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Code<S> {
        self.on_blame_open = Some(Rc::new(f));
        self
    }

    /// Called when the Send chip is pressed: send the snapshot to the terminal.
    pub fn on_send(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Code<S> {
        self.on_send = Some(Rc::new(f));
        self
    }

    /// Called on a secondary press in the code: the host opens its own menu,
    /// which replaces the field's built-in one.
    pub fn on_secondary(mut self, f: impl Fn(&mut S, PointerPress, &mut Tx) + 'static) -> Code<S> {
        self.on_secondary = Some(Rc::new(f));
        self
    }

    /// The unnamed Vim register shared across tabs. Without it yanks and
    /// pastes are no-ops (headless views and tests).
    pub fn register(mut self, f: fn(&mut S) -> &mut vim::Register) -> Code<S> {
        self.register = Some(f);
        self
    }

    /// `:w`, `:q` and `:wq`: the host saves and closes through its own guarded
    /// paths.
    pub fn on_vim_command(mut self, f: impl Fn(&mut S, VimCommand, &mut Tx) + 'static) -> Code<S> {
        self.on_vim_command = Some(Rc::new(f));
        self
    }

    /// First refusal on a key before either field's own handling: the host
    /// takes a chord the field would otherwise claim — `Tab` indents and
    /// `⌘⌥→` word-moves — `true` consumes the key.
    pub fn preempt_key(
        mut self,
        f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
    ) -> Code<S> {
        self.preempt_key = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &S, ink: &Ink, k: &Kit) -> El {
        let code = (self.get)(s);
        let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0).fill(ink.bg);
        if !code.conflicts.is_empty() {
            root = root.child(self.conflict_bar(code, ink));
        }
        if code.find.open {
            root = root.child(self.find_bar(s, ink));
        }
        if code.vim_cmd.is_some() {
            root = root.child(self.cmd_bar(s, ink));
        }
        if let Some(notice) = self.notice_bar(code, ink) {
            root = root.child(notice);
        }
        if let Some(failure) = self.blame_error_bar(code, ink) {
            root = root.child(failure);
        }
        let mut surface = row()
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .child(gutter_el(code, ink))
            .child(self.surface(s, ink, k));
        // The blame column is a sibling of the scroll surface: it scrolls
        // with the code. It stays mounted while the draft is dirty (blank),
        // or the code area would jump by the column's width on the first key.
        if code.blame_enabled {
            surface = surface.child(blame_el(code, ink, self.on_blame_open.clone()));
        }
        root.child(surface).child(self.footer(s, ink))
    }

    /// Find bar: query field, match count, case toggle, prev/next/close.
    fn find_bar(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let get_mut = self.get_mut;
        let preempt = self.preempt_key.clone();
        let mut bar = row()
            .w_full()
            .h(34.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .fill(ink.panel)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
        bar = bar.child(
            text_input(&code.find.query, QUERY_STYLE)
                .single_line()
                .placeholder("Find", ink.muted)
                .w(180.0)
                .color(ink.text)
                .caret(ink.text, ink.accent.alpha(0.3))
                .on_edit(move |st: &mut S| -> &mut TextEdit { &mut get_mut(st).find.query })
                .on_input_change(move |st: &mut S, tx: &mut Tx| {
                    let code = get_mut(st);
                    code.find.refresh(&code.edit.text);
                    if code.vim_search.is_some() {
                        return;
                    }
                    if let Some(r) = code.find.active_range() {
                        code.select_match(&r, tx);
                    }
                })
                .on_input_key(move |st: &mut S, ev: &KeyEvent, tx: &mut Tx| -> bool {
                    if preempt.as_ref().is_some_and(|f| f(st, ev, tx)) {
                        return true;
                    }
                    let code = get_mut(st);
                    match &ev.key {
                        Key::Escape => {
                            if code.vim_search.is_some() {
                                code.cancel_vim_search(tx);
                            } else {
                                code.close_find(tx);
                            }
                            true
                        }
                        Key::Enter => {
                            if code.vim_search.is_some() {
                                code.commit_vim_search(tx);
                            } else if ev.mods.shift {
                                code.find_prev(tx);
                            } else {
                                code.find_next(tx);
                            }
                            true
                        }
                        _ => false,
                    }
                }),
        );
        bar = bar.child(theme::txt(&code.find.count_label(), T12, ink.muted).min_w(58.0));
        let case = code.find.case_sensitive;
        bar = bar.child(
            theme::btn_sm("Aa", None, case, ink).on_press_tx(move |st, _tx| {
                let code = get_mut(st);
                code.find.case_sensitive = !code.find.case_sensitive;
                code.find.refresh(&code.edit.text);
            }),
        );
        bar = bar.child(
            theme::btn_sm("Prev", None, false, ink).on_press_tx(move |st, tx| {
                let _ = get_mut(st).find_prev(tx);
            }),
        );
        bar = bar.child(
            theme::btn_sm("Next", None, false, ink).on_press_tx(move |st, tx| {
                let _ = get_mut(st).find_next(tx);
            }),
        );
        bar.child(
            theme::btn_sm("Close", None, false, ink).on_press_tx(move |st, tx| {
                get_mut(st).close_find(tx);
            }),
        )
    }

    /// The Vim `:` bar: a prompt, the command input and an inline error.
    fn cmd_bar(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let Some(cmd) = &code.vim_cmd else {
            return col().w_full().h(0.0);
        };
        let get_mut = self.get_mut;
        let on_command = self.on_vim_command.clone();
        let mut bar = row()
            .w_full()
            .h(34.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .fill(ink.panel)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(theme::txt(":", T12, ink.muted));
        bar = bar.child(
            text_input(&cmd.input, QUERY_STYLE)
                .single_line()
                .key(VIM_CMD_KEY)
                .placeholder("Command", ink.muted)
                .w(260.0)
                .color(ink.text)
                .caret(ink.text, ink.accent.alpha(0.3))
                .on_edit(move |st: &mut S| -> &mut TextEdit {
                    let code = get_mut(st);
                    if code.vim_cmd.is_none() {
                        code.vim_cmd = Some(VimCmd::new());
                    }
                    &mut code.vim_cmd.as_mut().unwrap().input
                })
                .on_input_key(move |st: &mut S, ev: &KeyEvent, tx: &mut Tx| -> bool {
                    match &ev.key {
                        Key::Escape => {
                            get_mut(st).close_cmd_vim();
                            true
                        }
                        Key::Enter => {
                            let action = get_mut(st).submit_cmd_vim(tx);
                            if let (Some(action), Some(cb)) = (action, &on_command) {
                                cb(st, action, tx);
                            }
                            true
                        }
                        _ => false,
                    }
                }),
        );
        if let Some(error) = &cmd.error {
            bar = bar.child(theme::txt(error, T12, ink.warning));
        }
        bar
    }

    /// Slim status line for plain-text fallback and highlight caps.
    fn notice_bar(&self, code: &CodeState, ink: &Ink) -> Option<El> {
        let (msg, color) = if code.language().is_none() && !code.edit.text.is_empty() {
            (
                "Plain text — no syntax highlighter for this file type".to_string(),
                ink.muted,
            )
        } else {
            let r = code.active_syntax()?;
            let lang = r.language.as_deref().unwrap_or("text");
            if r.token_capped {
                (
                    format!("Syntax highlighting capped ({lang}) — the rest is plain text"),
                    ink.warning,
                )
            } else {
                return None;
            }
        };
        Some(
            row()
                .w_full()
                .h(22.0)
                .px(10.0)
                .items_center()
                .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
                .child(theme::txt(&msg, T11M_LH, color)),
        )
    }

    /// Slim status line for a failed blame read — an untracked path, a
    /// binary file, or a Git error leaves the column empty.
    fn blame_error_bar(&self, code: &CodeState, ink: &Ink) -> Option<El> {
        let error = code.blame_error.as_ref()?;
        Some(
            row()
                .w_full()
                .h(22.0)
                .px(10.0)
                .items_center()
                .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
                .child(theme::txt(
                    &format!("Blame unavailable — {error}"),
                    T11M_LH,
                    ink.warning,
                )),
        )
    }

    /// Scroll viewport + editor field(s) + optional go-to overlay.
    fn surface(&self, s: &S, ink: &Ink, k: &Kit) -> El {
        let code = (self.get)(s);
        let gutter_shift = code.gutter_shift.clone();
        let scroll_pos = code.scroll_pos.clone();
        let scroll_size = code.scroll_size.clone();
        let scroll = col()
            .key(SCROLL_KEY)
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .scroll_xy()
            .on_scroll_offset(move |at: ScrollPos| {
                gutter_shift.set([0.0, -at.offset[1]]);
                scroll_pos.set(at.offset);
                scroll_size.set(at.viewport);
            });
        // Fill blank space while retaining the scroll extent of longer documents.
        let mut wrap = col()
            .h_full()
            .min_h(code.line_count() as f32 * CODE_STYLE.line_px());
        if let Some(guides) = guides_el(code, ink) {
            wrap = wrap.child(guides);
        }
        wrap = wrap.child(self.field(s, ink));
        if let Some(caret) = vim_caret_el(code, ink) {
            wrap = wrap.child(caret);
        }
        if let Some(overlay) = self.go_to_overlay(code, ink) {
            wrap = wrap.child(overlay);
        }
        for block in &code.conflicts {
            let mut actions = row()
                .absolute(0.0, block.start_line as f32 * CODE_STYLE.line_px())
                .h(CODE_STYLE.line_px())
                .gap(4.0)
                .fill(ink.panel);
            for (label, choice) in [
                ("Accept Current", Choice::Current),
                ("Accept Incoming", Choice::Incoming),
                ("Accept Both", Choice::Both),
            ] {
                let block = block.clone();
                let revision = code.revision;
                let get_mut = self.get_mut;
                let changed = self.on_changed.clone();
                actions = actions.child(
                    theme::btn_sm(label, None, false, ink)
                        .key(format!("conflict-{}-{choice:?}", block.range.start))
                        .h(CODE_STYLE.line_px())
                        .disabled(code.read_only)
                        .on_press_tx(move |st: &mut S, tx| {
                            let code = get_mut(st);
                            if code.read_only || code.revision != revision {
                                return;
                            }
                            let replacement = block.replacement(&code.edit.text, choice);
                            code.edit.set_selection(block.range.start, block.range.end);
                            code.edit.insert(&replacement);
                            notify_changed(st, tx, get_mut, &changed);
                            get_mut(st).edit.focused = true;
                        }),
                );
            }
            wrap = wrap.child(actions);
        }
        scroll.child(wrap).scroll_bar(k)
    }

    fn conflict_bar(&self, code: &CodeState, ink: &Ink) -> El {
        let mut bar = row()
            .w_full()
            .min_h(30.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .flex_wrap()
            .fill(ink.panel);
        bar = bar.child(theme::txt(
            &format!("{} conflicts · Current / Incoming", code.conflicts.len()),
            T12,
            ink.warning,
        ));
        for (label, previous) in [("Previous conflict", true), ("Next conflict", false)] {
            let get_mut = self.get_mut;
            bar = bar.child(theme::btn_sm(label, None, false, ink).on_press_tx(
                move |st: &mut S, tx| {
                    let code = get_mut(st);
                    let caret = code.edit.focus;
                    let block = if previous {
                        code.conflicts
                            .iter()
                            .rev()
                            .find(|c| c.range.end <= caret)
                            .or_else(|| code.conflicts.last())
                    } else {
                        code.conflicts
                            .iter()
                            .find(|c| c.range.start > caret)
                            .or_else(|| code.conflicts.first())
                    };
                    if let Some(block) = block {
                        code.reveal(block.start_line as u32 + 1, tx);
                    }
                },
            ));
        }
        bar
    }

    /// Slim footer strip carrying the Vim mode, the blame toggle and the
    /// indent-unit chip. The settings are global, not per-document — the
    /// mode chip sits left, the rest right.
    fn footer(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let mut bar = row()
            .w_full()
            .h(22.0)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .justify_end()
            .border_sides([1.0, 0.0, 0.0, 0.0], ink.border);
        if code.vim.enabled && !code.read_only {
            let mode = code.vim.mode;
            bar = bar.child(theme::txt(
                mode.label(),
                T128,
                if mode.inserts() {
                    ink.muted
                } else {
                    ink.accent
                },
            ));
            let pending = code.vim.pending_label();
            if !pending.is_empty() {
                bar = bar.child(theme::txt(&pending, T128, ink.muted));
            }
            bar = bar.child(rune::spacer());
        }
        bar.child(self.send_chip(ink))
            .child(self.blame_chip(s, ink))
            .child(self.indent_chip(s, ink))
    }

    /// The Send chip: snapshot the selection (or the whole file) into the
    /// focused terminal session. The action reports its own outcome.
    fn send_chip(&self, ink: &Ink) -> El {
        let on_send = self.on_send.clone();
        row()
            .h(28.0)
            .px(10.0)
            .items_center()
            .radius(8.0)
            .child(theme::txt("Send", T128, ink.text))
            .hover(ink.text.alpha(0.08))
            .press(ink.text.alpha(0.12))
            .press_scale(0.97)
            .role("button")
            .label("Send snapshot to terminal")
            .focusable(true)
            .focus_ring(2.0, 1.0, ink.accent)
            .on_press_tx(move |st, tx| {
                if let Some(cb) = &on_send {
                    cb(st, tx);
                }
            })
    }

    /// The inline-blame toggle chip. The chip stays visible even when the
    /// column is hidden (a dirty draft hides the annotations, not the
    /// setting), so it carries the on/off state in its own colour.
    fn blame_chip(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let enabled = code.blame_enabled;
        let get = self.get;
        let on_blame = self.on_blame.clone();
        let color = if enabled { ink.accent } else { ink.muted };
        row()
            .h(28.0)
            .px(10.0)
            .items_center()
            .radius(8.0)
            .child(theme::txt("Blame", T128, color))
            .hover(ink.text.alpha(0.08))
            .press(ink.text.alpha(0.12))
            .press_scale(0.97)
            .role("button")
            .label("Blame")
            .focusable(true)
            .focus_ring(2.0, 1.0, ink.accent)
            .on_press_tx(move |st, tx| {
                let next = !(get)(st).blame_enabled;
                if let Some(cb) = &on_blame {
                    cb(st, next, tx);
                }
            })
    }

    /// The indent-unit chip: Auto → 2 → 4 → 8 → Tab, persisted globally.
    fn indent_chip(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let choice = code.indent_choice;
        let unit = code.indent_unit;
        let label = if choice == UnitChoice::Auto {
            format!("{} · auto", unit.label())
        } else {
            unit.label()
        };
        let get = self.get;
        let on_indent = self.on_indent.clone();
        theme::btn_sm(&label, None, false, ink).on_press_tx(move |st, tx| {
            let next = (get)(st).indent_choice.next();
            if let Some(cb) = &on_indent {
                cb(st, next, tx);
            }
        })
    }

    /// The editable `text_input` or the read-only selectable `text`, with
    /// syntax spans and find highlights.
    fn field(&self, s: &S, ink: &Ink) -> El {
        let code = (self.get)(s);
        let spans = self.syntax_spans(code, ink);
        let mut highlights = Vec::new();
        for block in &code.conflicts {
            highlights.push(TextHighlight::new(
                block.range.start..block.current.end,
                ink.success.alpha(0.16),
            ));
            highlights.push(TextHighlight::new(
                block.incoming.start..block.range.end,
                ink.accent.alpha(0.16),
            ));
        }
        highlights.extend(self.find_highlights(code, ink));
        if let Some(range) = code.vim_visual_range() {
            highlights.push(TextHighlight::new(range, ink.accent.alpha(0.25)));
        }
        if code.read_only {
            return text(code.edit.text.as_str())
                .font(CODE_STYLE)
                .color(ink.text)
                .key(TEXT_KEY)
                .selectable(true)
                .spans(spans)
                .text_highlights(highlights);
        }
        let get_mut = self.get_mut;
        let on_changed = self.on_changed.clone();
        let on_submit_changed = self.on_changed.clone();
        let on_changed_key = self.on_changed.clone();
        let on_save = self.on_save.clone();
        let preempt = self.preempt_key.clone();
        let register = self.register;
        // Outside Insert the Vim block caret is the only caret: the native
        // bar is drawn transparent under it.
        let block_caret = code.vim.enabled && !code.vim.mode.inserts() && code.edit.focused;
        let caret_color = if block_caret {
            Color::TRANSPARENT
        } else {
            ink.text
        };
        let mut field = text_input(&code.edit, CODE_STYLE)
            .grow(1.0)
            .color(ink.text)
            .caret(caret_color, ink.accent.alpha(0.3))
            .key(FIELD_KEY)
            .spans(spans)
            .text_highlights(highlights)
            .on_edit(move |st: &mut S| -> &mut TextEdit { &mut get_mut(st).edit })
            .on_input_change(move |st: &mut S, tx: &mut Tx| {
                // Native input (an IME commit, a paste) must not edit outside
                // Insert: the IME cannot be disabled at the pinned Rune rev, so
                // revert the change instead of letting Normal mode corrupt the
                // buffer.
                let code = get_mut(st);
                if code.vim.enabled && code.vim.mode != vim::Mode::Insert {
                    code.edit.undo();
                }
                notify_changed(st, tx, get_mut, &on_changed);
            })
            .on_input_key(move |st: &mut S, ev: &KeyEvent, tx: &mut Tx| -> bool {
                if preempt.as_ref().is_some_and(|f| f(st, ev, tx)) {
                    return true;
                }
                field_key(st, ev, tx, get_mut, register, &on_save, &on_changed_key)
            })
            .on_submit(move |st: &mut S, tx: &mut Tx| {
                // Enter keeps the line's indent (plus one level after an
                // opener) — the inserted text still routes through the same
                // change notification or revision/gutter/find state stales.
                {
                    let code = get_mut(st);
                    let (a, b) = code.edit.range();
                    let ins = indent::auto_indent(&code.edit.text, a.min(b), code.indent_unit);
                    code.edit.insert(&ins);
                }
                notify_changed(st, tx, get_mut, &on_submit_changed);
            });
        if let Some(cb) = self.on_secondary.clone() {
            field = field.on_secondary_press(move |st: &mut S, p, tx: &mut Tx| cb(st, p, tx));
        }
        field
    }

    /// Transparent link overlay mounted while the go-to modifier is held:
    /// identical text metrics inside the same scroll container, so its shaped
    /// glyph boxes line up exactly with the field below.
    fn go_to_overlay(&self, code: &CodeState, ink: &Ink) -> Option<El> {
        if !code.go_to_mod {
            return None;
        }
        let links = self.go_to_links(code, ink);
        if links.is_empty() {
            return None;
        }
        let (get, on_go_to) = (self.get, self.on_go_to.clone());
        let mut ov = text(code.edit.text.as_str())
            .font(CODE_STYLE)
            .color(Color::TRANSPARENT)
            .a11y_hidden()
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        for link in links {
            ov = ov.link(link);
        }
        // `.link()` sets `focus.enabled`, which is what lets Rune register
        // the links' shaped glyph boxes as pointer hits and route presses to
        // `on_link`. `tab_index(-1)` keeps the overlay out of Tab order, so
        // it never takes keyboard focus — the editor below keeps it.
        ov = ov.tab_index(-1);
        Some(ov.on_link(move |st: &mut S, id: &str, tx: &mut Tx| {
            if !tx.mods().cmd {
                return;
            }
            let mut parts = id.split(':');
            let (Some(from), Some(to)) = (
                parts.next().and_then(|n| n.parse::<usize>().ok()),
                parts.next().and_then(|n| n.parse::<usize>().ok()),
            ) else {
                return;
            };
            let Some(req) = get(st).go_to_token_at(from) else {
                return;
            };
            // The link id froze the token's byte range at render time; if
            // the text shifted under the overlay, the token now at `from`
            // spans a different range and the click is stale.
            if req.from != from || req.to != to {
                return;
            }
            if let Some(f) = &on_go_to {
                f(st, req, tx);
            }
        }))
    }

    /// Every go-to token outside suppressed trivia as a [`Link`].
    fn go_to_links(&self, code: &CodeState, ink: &Ink) -> Vec<Link> {
        let hover = LinkPaint::new().underline(ink.accent);
        code.go_to_ranges()
            .into_iter()
            .map(|r| Link::new(r.clone(), format!("{}:{}", r.start, r.end)).hover(hover))
            .collect()
    }

    /// Colour-only spans for the revision-matched syntax result.
    fn syntax_spans(&self, code: &CodeState, ink: &Ink) -> Vec<Span> {
        let Some(r) = code.active_syntax() else {
            return Vec::new();
        };
        syntax_theme(ink).spans(&r.ranges, 0)
    }

    /// Subtle wash on every match, a stronger keyed one on the active match.
    fn find_highlights(&self, code: &CodeState, ink: &Ink) -> Vec<TextHighlight> {
        let active = code.find.active_range();
        let mut out = Vec::with_capacity(MAX_MATCH_HIGHLIGHTS + 1);
        let mut active_pushed = false;
        for (i, r) in code.find.matches.iter().enumerate() {
            if i >= MAX_MATCH_HIGHLIGHTS {
                break;
            }
            if active.as_ref() == Some(r) {
                out.push(TextHighlight::new(r.clone(), ink.accent.alpha(0.38)).key(FIND_MATCH_KEY));
                active_pushed = true;
            } else {
                out.push(TextHighlight::new(r.clone(), ink.accent.alpha(0.14)));
            }
        }
        if !active_pushed && let Some(r) = &active {
            out.push(TextHighlight::new(r.clone(), ink.accent.alpha(0.38)).key(FIND_MATCH_KEY));
        }
        out
    }
}

/// The single post-mutation notification path shared by `on_input_change`
/// and `on_submit`: resync revision/gutter/find via [`CodeState::text_edited`],
/// then fire the parent's `on_changed`.
fn notify_changed<S: 'static>(
    st: &mut S,
    tx: &mut Tx,
    get_mut: fn(&mut S) -> &mut CodeState,
    on_changed: &Option<StateCallback<S>>,
) {
    get_mut(st).text_edited();
    if let Some(cb) = on_changed {
        cb(st, tx);
    }
}

/// Editor-field shortcuts, handled before the native editor sees the key:
/// ⌘S save, ⌘F find, ⌘G/⇧⌘G match navigation, Esc closes the find bar, and
/// Tab/⇧Tab line indentation via [`indent::plan_indent`].
fn field_key<S: 'static>(
    st: &mut S,
    ev: &KeyEvent,
    tx: &mut Tx,
    get_mut: fn(&mut S) -> &mut CodeState,
    register: Option<fn(&mut S) -> &mut vim::Register>,
    on_save: &Option<StateCallback<S>>,
    on_changed: &Option<StateCallback<S>>,
) -> bool {
    if let Some(key) = vim_key_for(st, ev, get_mut) {
        let mode_before = get_mut(st).vim.mode;
        let effects = {
            let code = get_mut(st);
            let indent = code.indent_unit;
            let CodeState { edit, vim, .. } = code;
            let doc = vim::Doc {
                text: &edit.text,
                focus: edit.focus,
                indent,
            };
            vim::step(vim, &doc, key)
        };
        if apply_vim_effects(st, tx, get_mut, register, on_changed, effects, mode_before) {
            return true;
        }
    }
    if ev.is_command('s') {
        if let Some(cb) = on_save {
            cb(st, tx);
        }
        return true;
    }
    if ev.is_command('/') {
        let code = get_mut(st);
        let style = comment::style_for(&code.path);
        let (a, b) = code.edit.range();
        if let (false, Some(style)) = (code.read_only, style)
            && let Some(e) = comment::plan_toggle(&code.edit.text, a, b, style)
        {
            code.edit.set_selection(e.start, e.end);
            code.edit.insert(&e.text);
            code.edit.set_selection(e.anchor, e.focus);
            notify_changed(st, tx, get_mut, on_changed);
        }
        return true;
    }
    // A Tab with cmd/ctrl/alt is a chord (⌃Tab switches tabs) — Shift is
    // the only modifier indent still takes, for outdent.
    if ev.key == Key::Tab && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt {
        let code = get_mut(st);
        let (a, b) = code.edit.range();
        if let Some(e) = indent::plan_indent(&code.edit.text, a, b, code.indent_unit, ev.mods.shift)
        {
            code.edit.set_selection(e.start, e.end);
            code.edit.insert(&e.text);
            code.edit.set_selection(e.anchor, e.focus);
            notify_changed(st, tx, get_mut, on_changed);
        }
        return true;
    }
    let code = get_mut(st);
    if ev.is_command('f') {
        code.open_find(tx);
        return true;
    }
    if ev.is_command('g') {
        let hit = if ev.mods.shift {
            code.find_prev(tx)
        } else {
            code.find_next(tx)
        };
        return hit.is_some();
    }
    if ev.key == Key::Escape && code.find.open {
        code.close_find(tx);
        return true;
    }
    false
}

/// The key the Vim engine sees, or `None` when the field is not routed (Vim
/// off, read-only) or the chord belongs to the app.
fn vim_key_for<S: 'static>(
    st: &mut S,
    ev: &KeyEvent,
    get_mut: fn(&mut S) -> &mut CodeState,
) -> Option<vim::Key> {
    let code = get_mut(st);
    if !code.vim.enabled || code.read_only {
        return None;
    }
    vim_key(ev)
}

/// `rune::KeyEvent` as the engine's own key. Cmd and Option chords return
/// `None` so every app shortcut keeps working; Control reaches the engine as
/// [`vim::Key::Ctrl`] (redo).
fn vim_key(ev: &KeyEvent) -> Option<vim::Key> {
    if ev.mods.alt {
        return None;
    }
    if ev.mods.ctrl {
        let c = match &ev.key {
            Key::Text(s) => s.chars().next()?,
            Key::Command(c) => *c,
            _ => return None,
        };
        return Some(vim::Key::Ctrl(c.to_ascii_lowercase()));
    }
    if ev.mods.cmd {
        return None;
    }
    Some(match &ev.key {
        Key::Text(s) if s.chars().count() == 1 => vim::Key::Char(s.chars().next()?),
        Key::Enter => vim::Key::Enter,
        Key::Escape => vim::Key::Escape,
        Key::Backspace => vim::Key::Backspace,
        Key::Delete => vim::Key::Delete,
        Key::Tab => vim::Key::Tab,
        Key::Left => vim::Key::Left,
        Key::Right => vim::Key::Right,
        Key::Up => vim::Key::Up,
        Key::Down => vim::Key::Down,
        Key::Home => vim::Key::Home,
        Key::End => vim::Key::End,
        Key::PageUp => vim::Key::PageUp,
        Key::PageDown => vim::Key::PageDown,
        _ => return None,
    })
}

/// Apply the engine's effects to the shared `TextEdit`. `false` hands the key
/// to the native field (Insert-mode typing). Every text change here is one
/// undo step; the caller wraps nothing else.
fn apply_vim_effects<S: 'static>(
    st: &mut S,
    tx: &mut Tx,
    get_mut: fn(&mut S) -> &mut CodeState,
    register: Option<fn(&mut S) -> &mut vim::Register>,
    on_changed: &Option<StateCallback<S>>,
    effects: Vec<vim::Effect>,
    mode_before: vim::Mode,
) -> bool {
    let mut changed = false;
    let mut select: Option<(usize, usize)> = None;
    let mut texts: Vec<(usize, usize, String)> = Vec::new();
    let mut pastes: Vec<(bool, usize)> = Vec::new();
    let mut undo = false;
    let mut redo = false;
    for effect in effects {
        match effect {
            vim::Effect::Pass => return false,
            vim::Effect::Select { anchor, focus } => select = Some((anchor, focus)),
            vim::Effect::Replace { start, end, text } => texts.push((start, end, text)),
            vim::Effect::Yank { text, linewise } => {
                if let Some(f) = register {
                    f(st).set(text.clone(), linewise);
                }
                tx.copy(text);
            }
            vim::Effect::Paste { before, count } => pastes.push((before, count)),
            vim::Effect::Undo => undo = true,
            vim::Effect::Redo => redo = true,
            vim::Effect::Find(action) => {
                let code = get_mut(st);
                match action {
                    vim::FindAction::Open { forward } => code.open_find_vim(forward, tx),
                    vim::FindAction::Next => {
                        code.find_step_vim(true, tx);
                    }
                    vim::FindAction::Prev => {
                        code.find_step_vim(false, tx);
                    }
                    vim::FindAction::WordUnderCaret => code.search_word_vim(tx),
                }
            }
            vim::Effect::Command(vim::CommandAction::Open) => get_mut(st).open_cmd_vim(),
            vim::Effect::Scroll(action) => get_mut(st).scroll_line(action, tx),
        }
    }
    if undo {
        get_mut(st).edit.undo();
        changed = true;
    }
    if redo {
        get_mut(st).edit.redo();
        changed = true;
    }
    for (before, count) in pastes {
        let reg = register.map(|f| f(st).clone()).unwrap_or_default();
        if reg.is_empty() {
            request_clipboard_paste(st, tx, get_mut, on_changed, before, count);
            continue;
        }
        let at = get_mut(st).edit.focus;
        if let Some((start, end, insert, caret)) =
            paste_edit_at(get_mut(st), &reg, at, before, count)
        {
            let code = get_mut(st);
            code.edit.undo_group(|edit| {
                edit.set_selection(start, end);
                edit.insert(&insert);
            });
            code.edit.set_selection(caret, caret);
            changed = true;
        }
    }
    if !texts.is_empty() {
        let code = get_mut(st);
        code.edit.undo_group(|edit| {
            for (start, end, text) in &texts {
                edit.set_selection(*start, *end);
                edit.insert(text);
            }
        });
        changed = true;
    }
    if let Some((anchor, focus)) = select {
        let code = get_mut(st);
        code.edit.set_selection(anchor, focus);
        code.scroll_to_caret(tx);
    }
    // An Insert session: capture where it starts, then on exit recover the net
    // inserted text for `.` (a pure insertion still ends with the entry tail).
    let code = get_mut(st);
    if mode_before != vim::Mode::Insert && code.vim.mode == vim::Mode::Insert {
        let at = code.edit.focus.min(code.edit.text.len());
        code.vim_insert = Some((at, code.edit.text[at..].to_string()));
    } else if mode_before == vim::Mode::Insert && code.vim.mode != vim::Mode::Insert {
        code.edit.break_history_group();
        if let Some((start, tail)) = code.vim_insert.take() {
            let text = &code.edit.text;
            if start <= text.len() && text[start..].ends_with(&tail) {
                let inserted = text[start..text.len() - tail.len()].to_string();
                code.vim.record_insert(&inserted);
            }
        }
    }
    if changed {
        notify_changed(st, tx, get_mut, on_changed);
    }
    true
}

/// The byte after `at`, or `at` at the end.
fn next_char(text: &str, at: usize) -> usize {
    text.get(at..)
        .and_then(|rest| rest.chars().next())
        .map_or(at, |c| at + c.len_utf8())
}

/// The edit a register paste makes at `at`: `(start, end, text, caret)`.
fn paste_edit_at(
    code: &CodeState,
    reg: &vim::Register,
    at: usize,
    before: bool,
    count: usize,
) -> Option<(usize, usize, String, usize)> {
    if reg.is_empty() {
        return None;
    }
    let text = &code.edit.text;
    let at = at.min(text.len());
    let line = code.edit.line_of_byte(at);
    let line_range = code.edit.line_range(line);
    let (start, insert) = if reg.linewise {
        let mut insert = reg.text.clone();
        if !insert.ends_with('\n') {
            insert.push('\n');
        }
        if before {
            (line_range.start, insert)
        } else {
            let next = if line_range.end < text.len() {
                line_range.end + 1
            } else {
                line_range.end
            };
            if next == text.len() && !text.is_empty() && !text.ends_with('\n') {
                insert.insert(0, '\n');
            }
            (next, insert)
        }
    } else {
        let pos = if before || at >= line_range.end {
            at
        } else {
            next_char(text, at)
        };
        (pos, reg.text.clone())
    };
    let insert = insert.repeat(count);
    let caret = if reg.linewise {
        start
    } else {
        let last = insert.chars().next_back().map_or(0, |c| c.len_utf8());
        start + insert.len() - last
    };
    Some((start, start, insert, caret))
}

/// `p`/`P` with an empty register: read the system clipboard and paste when it
/// arrives, as its own undo step.
fn request_clipboard_paste<S: 'static>(
    st: &mut S,
    tx: &mut Tx,
    get_mut: fn(&mut S) -> &mut CodeState,
    on_changed: &Option<StateCallback<S>>,
    before: bool,
    count: usize,
) {
    let at = get_mut(st).edit.focus;
    let changed_cb = on_changed.clone();
    tx.read_clipboard(move |st: &mut S, content, tx| {
        let text = content.text().to_string();
        if text.is_empty() {
            return;
        }
        let linewise = text.ends_with('\n');
        let reg = vim::Register { text, linewise };
        let Some((start, end, insert, caret)) = paste_edit_at(get_mut(st), &reg, at, before, count)
        else {
            return;
        };
        let code = get_mut(st);
        code.edit.undo_group(|edit| {
            edit.set_selection(start, end);
            edit.insert(&insert);
        });
        code.edit.set_selection(caret, caret);
        notify_changed(st, tx, get_mut, &changed_cb);
    });
}

/// Visual columns of `s`: a tab advances to the next 4-column stop, every
/// other character takes one cell (the monospace model the guides use).
fn display_cols(s: &str) -> usize {
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            col = (col / indent::TAB_STOP as usize + 1) * indent::TAB_STOP as usize;
        } else {
            col += 1;
        }
    }
    col
}

/// The Vim block caret: one translucent rectangle over the character under
/// the caret (a cell at line end or on an empty line), in document
/// coordinates inside the scroll content so it scrolls with the text. The
/// native bar is drawn transparent while this is mounted.
fn vim_caret_el(code: &CodeState, ink: &Ink) -> Option<El> {
    if !code.vim.enabled || code.vim.mode.inserts() || !code.edit.focused || code.read_only {
        return None;
    }
    let caret = code.edit.focus.min(code.edit.text.len());
    let line = code.edit.line_of_byte(caret);
    let line_range = code.edit.line_range(line);
    let line_px = CODE_STYLE.line_px();
    let cell = rune::text::advance("0", CODE_STYLE).max(1.0);
    let cols = display_cols(&code.edit.text[line_range.start..caret]);
    let x = cols as f32 * cell;
    let next = code
        .edit
        .text
        .get(caret..)
        .and_then(|rest| rest.chars().next());
    let w = match next {
        Some('\t') => {
            let stop = (cols / indent::TAB_STOP as usize + 1) * indent::TAB_STOP as usize;
            (stop - cols) as f32 * cell
        }
        Some(_) if caret < line_range.end => cell,
        _ => cell,
    };
    let y = line as f32 * line_px;
    let shape = Shape::new().fill(
        [[x, y], [x + w, y], [x + w, y + line_px], [x, y + line_px]],
        ink.text.alpha(0.28),
    );
    Some(
        canvas(shape)
            .key(VIM_CARET_KEY)
            .absolute(0.0, 0.0)
            .size(x + w, y + line_px)
            .a11y_hidden(),
    )
}

/// Indent guides: merged vertical ticks in document coordinates inside one
/// canvas, mounted under the field in the scroll content so scrolling moves
/// them natively. Ticks sit at each multiple of the indent unit a line's
/// leading whitespace reaches; runs merge across contiguous lines so a
/// block draws as a handful of segments, not one op per line.
fn guides_el(code: &CodeState, ink: &Ink) -> Option<El> {
    let lines = code.line_count();
    if lines == 0 || lines > indent::MAX_GUIDE_LINES {
        return None;
    }
    let cell_w = rune::text::advance("0", CODE_STYLE);
    if cell_w <= 0.0 {
        return None;
    }
    let cols = indent::indent_columns(&code.edit.text);
    let runs = indent::guide_runs(&cols, code.indent_unit);
    if runs.is_empty() {
        return None;
    }
    let line_px = CODE_STYLE.line_px();
    let mut shape = Shape::new();
    let mut max_x = 0.0f32;
    for (g, first, last) in runs {
        // Half-pixel x keeps the 1px stroke centred on the column boundary.
        let x = g as f32 * cell_w + 0.5;
        shape = shape.line(
            [x, first as f32 * line_px + 1.0],
            [x, (last as f32 + 1.0) * line_px - 1.0],
            1.0,
            LineCap::Butt,
            ink.border,
        );
        max_x = max_x.max(x);
    }
    Some(
        canvas(shape)
            .absolute(0.0, 0.0)
            .size(max_x + cell_w, lines as f32 * line_px)
            .a11y_hidden(),
    )
}

/// Change bars beside the line numbers: a bar per added (green) or modified
/// (blue) span and a short red tick where lines were removed, in the same
/// document coordinates the numbers use.
fn change_bars_el(code: &CodeState, ink: &Ink, width: f32) -> Option<El> {
    if !code.changes_visible || code.line_changes.is_empty() {
        return None;
    }
    let line_px = CODE_STYLE.line_px();
    let x = width - 4.0;
    let mut shape = Shape::new();
    for change in &code.line_changes {
        let top = (change.start.saturating_sub(1)) as f32 * line_px;
        shape = match change.kind {
            "deleted" => shape.line(
                [x - 3.0, top],
                [x + 3.0, top],
                2.0,
                LineCap::Butt,
                ink.danger,
            ),
            kind => {
                let color = if kind == "added" {
                    ink.success
                } else {
                    ink.accent
                };
                shape.line(
                    [x, top],
                    [x, top + change.count as f32 * line_px],
                    3.0,
                    LineCap::Butt,
                    color,
                )
            }
        };
    }
    Some(
        canvas(shape)
            .absolute(0.0, 0.0)
            .size(width, code.line_count() as f32 * line_px)
            .live_offset(&code.gutter_shift)
            .a11y_hidden(),
    )
}

/// Line-number gutter: one right-aligned numbers text shifted by the live
/// scroll offset, clipped to the gutter box.
fn gutter_el(code: &CodeState, ink: &Ink) -> El {
    let shift = code.gutter_shift.clone();
    let width = code.gutter_width();
    let mut gutter = col()
        .w(width)
        .h_full()
        .fill(ink.panel)
        .border_sides([0.0, 1.0, 0.0, 0.0], ink.border)
        .clip_rounded(0.0)
        .child(
            text(code.gutter_text())
                .font(CODE_STYLE)
                .color(ink.muted)
                .pl(10.0)
                .pr(10.0)
                .live_offset(&shift),
        );
    if let Some(bars) = change_bars_el(code, ink, width) {
        gutter = gutter.child(bars);
    }
    gutter
}

/// Inline-blame column: the caret line's annotation, shifted by the same live
/// scroll offset the gutter uses so it lines up with its line. It is placed by
/// line height, not by padding the text with blank rows, so building it costs
/// the same on a 10-line and a 10 000-line file.
fn blame_el<S: 'static>(code: &CodeState, ink: &Ink, on_open: Option<BlameOpenCallback<S>>) -> El {
    let shift = code.gutter_shift.clone();
    let bytes = code.edit.text.as_bytes();
    let active = bytes[..code.edit.focus.min(bytes.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count();
    let number = active as u64 + 1;
    let shown = code.blame.as_ref().filter(|_| code.blame_visible);
    let label = match shown {
        Some(blame) => blame::line_label(blame, number, blame::now_ms()),
        None => String::new(),
    };
    let mut line = row().w_full().h(CODE_STYLE.line_px()).child(
        text(label)
            .font(CODE_STYLE)
            .color(ink.muted)
            .pl(10.0)
            .pr(10.0),
    );
    if let (Some(commit), Some(open)) = (shown.and_then(|b| blame::commit_of(b, number)), on_open) {
        let commit = commit.to_string();
        line = line
            .hover(ink.text.alpha(0.08))
            .press(ink.text.alpha(0.12))
            .role("button")
            .label("Open commit in History")
            .on_press_tx(move |st: &mut S, tx| open(st, &commit, tx));
    }
    col()
        .w(BLAME_WIDTH)
        .h_full()
        .fill(ink.bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], ink.border)
        .clip_rounded(0.0)
        .label("Blame annotations")
        .child(
            // A spacer carries the line's offset so the press area is just the
            // line, not the column above it.
            col()
                .child(el().h(active as f32 * CODE_STYLE.line_px()))
                .child(line)
                .live_offset(&shift),
        )
}

/// Token hues on the editor surface: VS Code Dark+'s on a dark palette,
/// Light+'s on a light one — the Dark+ hues were unreadable on paper.
/// Status-ink kinds (punctuation, diffs, parameters) follow the palette.
fn syntax_theme(ink: &Ink) -> SyntaxTheme {
    let theme = SyntaxTheme::neutral(ink.dark);
    if ink.dark {
        theme
            .with(Syntax::Keyword, theme::c("#569cd6"))
            .with(Syntax::String, theme::c("#ce9178"))
            .with(Syntax::Number, theme::c("#b5cea8"))
            .with(Syntax::Comment, theme::c("#6a9955"))
            .with(Syntax::Type, theme::c("#4ec9b0"))
            .with(Syntax::Function, theme::c("#dcdcaa"))
            .with(Syntax::Property, theme::c("#9cdcfe"))
            .with(Syntax::Punctuation, ink.muted)
            .with(Syntax::Inserted, ink.success)
            .with(Syntax::Deleted, ink.danger)
            .with(Syntax::Parameter, ink.text)
    } else {
        theme
            .with(Syntax::Keyword, theme::c("#0000ff"))
            .with(Syntax::String, theme::c("#a31515"))
            .with(Syntax::Number, theme::c("#087a51"))
            .with(Syntax::Comment, theme::c("#008000"))
            .with(Syntax::Type, theme::c("#1f6f84"))
            .with(Syntax::Function, theme::c("#795e26"))
            .with(Syntax::Property, theme::c("#001080"))
            .with(Syntax::Punctuation, ink.muted)
            .with(Syntax::Inserted, ink.success)
            .with(Syntax::Deleted, ink.danger)
            .with(Syntax::Parameter, ink.text)
    }
}

/// Whether a go-to token is an import target (specifier or imported name) —
/// the one token kind allowed inside string trivia.
fn is_import(target: &gtd::GoToTarget) -> bool {
    matches!(target, gtd::GoToTarget::Import(_))
}

/// Whether `[from, to)` overlaps trivia that suppresses this token.
/// Desktop `tokenUnder` parity: only `Name` tokens inside comments or
/// strings are filtered — import targets bypass trivia entirely, so a
/// commented-out import still links. `trivia` is sorted, non-overlapping,
/// and whole-document (independent of the syntax colour budget).
fn token_blocked(
    trivia: &[(Range<usize>, syntax::Trivia)],
    is_import: bool,
    from: usize,
    to: usize,
) -> bool {
    if is_import {
        return false;
    }
    let i = trivia.partition_point(|(r, _)| r.end <= from);
    trivia[i..]
        .iter()
        .take_while(|(r, _)| r.start < to)
        .next()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(path: &str, text: &str) -> CodeState {
        CodeState::new(path, text)
    }

    /// The WCAG contrast ratio of two opaque colours.
    fn contrast(a: rune::Color, b: rune::Color) -> f32 {
        let (hi, lo) = (
            a.luminance().max(b.luminance()),
            a.luminance().min(b.luminance()),
        );
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn the_light_palette_keeps_tokens_readable() {
        let ink = Ink::for_theme(theme::Theme::Light);
        let theme = syntax_theme(&ink);
        for kind in [
            Syntax::Keyword,
            Syntax::String,
            Syntax::Number,
            Syntax::Comment,
            Syntax::Type,
            Syntax::Function,
            Syntax::Property,
        ] {
            let ratio = contrast(theme.color(kind), ink.bg);
            assert!(
                ratio >= 4.5,
                "{kind:?} reads at only {ratio:.2}:1 on the light surface"
            );
        }
    }

    #[test]
    fn the_dark_palette_keeps_the_darker_hues() {
        let ink = Ink::new();
        let theme = syntax_theme(&ink);
        assert_eq!(theme.color(Syntax::Keyword), theme::c("#569cd6"));
        assert_eq!(theme.color(Syntax::Comment), theme::c("#6a9955"));
        assert_eq!(theme.color(Syntax::Function), theme::c("#dcdcaa"));
    }

    #[test]
    fn load_and_edit_bump_revision_and_gutter() {
        let mut c = state("a.rs", "fn main() {}\nfn two() {}");
        assert_eq!(c.revision, 1);
        assert_eq!(c.line_count(), 2);
        assert_eq!(c.gutter_text(), "1\n2");
        assert_eq!(c.gutter_width(), 20.0 + 8.0);
        c.edit.insert("x");
        c.text_edited();
        assert_eq!(c.revision, 2);
        c.load("b.rs", "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12");
        assert_eq!(c.revision, 3);
        assert_eq!(c.line_count(), 12);
        assert!(c.gutter_text().starts_with(" 1\n 2"));
        assert!(c.gutter_text().ends_with("12"));
        assert_eq!(c.gutter_width(), 20.0 + 2.0 * 8.0);
    }

    #[test]
    fn reveal_clamps_lines_and_centres() {
        let c = state("a.txt", "aaa\nbbb\nccc");
        assert_eq!(c.clamp_line(0), 0, "0 clamps to the first line");
        assert_eq!(c.clamp_line(1), 0);
        assert_eq!(c.clamp_line(3), 2);
        assert_eq!(c.clamp_line(99), 2, "past the end clamps to the last line");
        let line_px = CODE_STYLE.line_px();
        assert_eq!(
            c.reveal_scroll_y(0, 38.0),
            0.0,
            "never scrolls above the top"
        );
        // Line 3 in a 38 pt viewport (two lines visible): top = 2*19 = 38.
        let y = c.reveal_scroll_y(2, 38.0);
        assert!((y - (38.0 + line_px * 0.5 - 19.0)).abs() < f32::EPSILON);
        // The clamped line's byte range is what gets selected.
        let idx = c.clamp_line(99);
        assert_eq!(&c.edit.text[c.edit.line_range(idx)], "ccc");
    }

    #[test]
    fn vim_key_maps_plain_keys_and_rejects_app_chords() {
        let plain = |key| KeyEvent {
            key,
            mods: rune::Mods::default(),
        };
        assert_eq!(
            vim_key(&plain(Key::Text("w".into()))),
            Some(vim::Key::Char('w'))
        );
        assert_eq!(vim_key(&plain(Key::Escape)), Some(vim::Key::Escape));
        assert_eq!(vim_key(&plain(Key::Enter)), Some(vim::Key::Enter));
        assert_eq!(vim_key(&plain(Key::Tab)), Some(vim::Key::Tab));
        let cmd = rune::Mods {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(
            vim_key(&KeyEvent {
                key: Key::Text("s".into()),
                mods: cmd
            }),
            None,
            "⌘S stays the app's"
        );
        let ctrl = rune::Mods {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            vim_key(&KeyEvent {
                key: Key::Text("r".into()),
                mods: ctrl
            }),
            Some(vim::Key::Ctrl('r'))
        );
        let alt = rune::Mods {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            vim_key(&KeyEvent {
                key: Key::Text("l".into()),
                mods: alt
            }),
            None
        );
    }

    #[test]
    fn find_state_lives_in_code_state() {
        let mut c = state("a.txt", "foo bar foo");
        c.find.set_query("foo");
        c.find.refresh(&c.edit.text);
        assert_eq!(c.find.matches.len(), 2);
        c.text_edited(); // refresh keeps matches after unrelated edit path
        assert_eq!(c.find.matches.len(), 2);
    }

    #[test]
    fn syntax_result_applies_only_at_current_revision() {
        let mut c = state("a.rs", "fn main() {}");
        c.syntax = Some(HighlightResult {
            revision: c.revision,
            language: Some("rust".into()),
            token_capped: false,
            ranges: vec![(
                0..2,
                rune_kit::syntax::Paint::Kind(rune_kit::syntax::Syntax::Keyword),
            )],
        });
        assert!(c.active_syntax().is_some());
        c.edit.insert("x");
        c.text_edited();
        assert!(c.active_syntax().is_none(), "stale revision must not apply");
    }

    #[test]
    fn go_to_token_at_maps_document_bytes() {
        let c = state("a.ts", "import { x } from './util'\nconst v = sumAll(1)");
        let line1 = "import { x } from './util'";
        let at = line1.find("util").unwrap();
        assert!(matches!(
            c.go_to_token_at(at).unwrap().target,
            gtd::GoToTarget::Import(_)
        ));
        let line2_off = line1.len() + 1;
        let at2 = line2_off + "const v = ".len();
        let req = c.go_to_token_at(at2).unwrap();
        assert_eq!(req.target, gtd::GoToTarget::Name("sumAll".into()));
        assert_eq!((req.from, req.to), (at2, at2 + 6));
        // Past the end clamps instead of panicking.
        let _ = c.go_to_token_at(usize::MAX);
    }

    #[test]
    fn go_to_ranges_skip_comments_and_strings() {
        let mut c = state(
            "a.js",
            "// sumAll here\nconst s = \"sumAll\";\nconst t = sumAll(x)",
        );
        c.syntax = Some(crate::editor::syntax::highlight_revision(
            c.revision,
            Some("js"),
            &c.edit.text,
        ));
        let ranges = c.go_to_ranges();
        let text = c.edit.text.as_str();
        // Exactly one 'sumAll' links: the call on the last line.
        let linked: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(
            linked,
            vec!["sumAll"],
            "comment and string occurrences must not link"
        );
        assert_eq!(ranges[0].start, text.rfind("sumAll").unwrap());
        // Exclusion comes from the lexical classifier, not the colour
        // result: clearing the syntax result changes nothing.
        c.syntax = None;
        c.revision += 1;
        assert_eq!(c.go_to_ranges(), ranges);
    }

    #[test]
    fn go_to_ranges_covers_imports_and_names() {
        let c = state(
            "a.ts",
            "import { sum } from './sum'\nexport const total = sum(1)",
        );
        let ranges = c.go_to_ranges();
        let text = c.edit.text.as_str();
        let linked: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        // 'sum' inside './sum' is shadowed by the specifier token itself.
        assert!(linked.contains(&"./sum"));
        assert!(linked.contains(&"sum"));
        assert!(linked.contains(&"total"));
        // Keywords never link.
        assert!(!linked.contains(&"import"));
        assert!(!linked.contains(&"const"));
    }

    #[test]
    fn go_to_ranges_keeps_import_specifier_inside_string_trivia() {
        // Real syntax marks './greet' a String; import specifiers are strings
        // syntactically and must still link (desktop never trivia-filters
        // import targets).
        let mut c = state("a.js", "import { greet } from './greet'\nconst x = 1");
        c.syntax = Some(syntax::highlight_revision(
            c.revision,
            c.language(),
            &c.edit.text,
        ));
        let ranges = c.go_to_ranges();
        let text = c.edit.text.as_str();
        let linked: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert!(
            linked.contains(&"./greet"),
            "import specifier must link inside String trivia: {linked:?}"
        );
        assert!(
            linked.contains(&"greet"),
            "the imported name still links: {linked:?}"
        );
    }

    #[test]
    fn go_to_token_at_ignores_comment_and_string_names() {
        // token_at must classify like the pointer overlay: names inside
        // comments/strings are not go-to targets; code names still are.
        let mut c = state(
            "a.js",
            "// sumAll\nconst s = \"sumAll\"\nconst t = sumAll(x)",
        );
        c.syntax = Some(syntax::highlight_revision(
            c.revision,
            c.language(),
            &c.edit.text,
        ));
        let text = c.edit.text.as_str();
        let comment_at = text.find("sumAll").unwrap();
        let string_at = text.match_indices("sumAll").nth(1).unwrap().0;
        let call_at = text.rfind("sumAll").unwrap();
        assert!(
            c.go_to_token_at(comment_at).is_none(),
            "comment name must not resolve"
        );
        assert!(
            c.go_to_token_at(string_at).is_none(),
            "string name must not resolve"
        );
        let req = c.go_to_token_at(call_at).expect("call-site name resolves");
        assert_eq!(req.target, gtd::GoToTarget::Name("sumAll".into()));
        assert_eq!((req.from, req.to), (call_at, call_at + 6));
    }

    #[test]
    fn go_to_ranges_excludes_trivia_without_syntax_result() {
        // No syntax result (pending, stale or plain-text path): the lexical
        // classifier still suppresses comment/string names.
        let c = state(
            "a.js",
            "// hiddenName\nconst s = \"strName\"\nvisibleName(x)",
        );
        assert!(c.active_syntax().is_none());
        let ranges = c.go_to_ranges();
        let text = c.edit.text.as_str();
        let linked: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(
            linked,
            vec!["visibleName"],
            "comment and string names must not link: {linked:?}"
        );
    }

    #[test]
    fn go_to_ranges_excludes_trivia_on_large_file() {
        // Past the retired 32 KiB colour prefix the file now highlights in
        // full, and comment/string exclusion keeps working document-wide.
        let mut text = String::new();
        while text.len() < 40 * 1024 {
            text.push_str("let value = 1;\n");
        }
        text.push_str("// trailingCommentName\nlet afterCapName = 1;\n");
        let mut c = state("a.js", &text);
        c.syntax = Some(syntax::highlight_revision(
            c.revision,
            c.language(),
            &c.edit.text,
        ));
        assert!(!c.active_syntax().unwrap().token_capped);
        let ranges = c.go_to_ranges();
        let doc = c.edit.text.as_str();
        for r in &ranges {
            let piece = &doc[r.clone()];
            assert!(
                !piece.contains("trailingCommentName"),
                "comment name past the cap must not link"
            );
        }
        assert!(
            ranges.iter().any(|r| &doc[r.clone()] == "afterCapName"),
            "code past the cap still links"
        );
    }

    #[test]
    fn go_to_ranges_has_no_hidden_prefix_cap() {
        // Every token links: a target near the end of a large file is not cut
        // off by an earlier-token budget.
        let mut text = String::new();
        for i in 0..5000 {
            text.push_str(&format!("alphaName{i} = 1;\n"));
        }
        text.push_str("targetNameAtEnd(x)\n");
        let c = state("a.js", &text);
        let ranges = c.go_to_ranges();
        let doc = c.edit.text.as_str();
        assert!(
            ranges.iter().any(|r| &doc[r.clone()] == "targetNameAtEnd"),
            "late target must link"
        );
        assert!(
            ranges.len() > 5000,
            "no silent first-N cutoff: {}",
            ranges.len()
        );
    }

    #[test]
    fn token_blocked_respects_trivia_kinds() {
        use syntax::Trivia::*;
        let t = vec![(5..10, Comment), (20..30, String)];
        // Desktop parity: import targets bypass trivia entirely — a
        // commented-out import still links.
        assert!(!token_blocked(&t, true, 7, 12));
        assert!(!token_blocked(&t, true, 4, 6));
        assert!(!token_blocked(&t, true, 20, 21));
        // Names inside either trivia kind are suppressed.
        assert!(token_blocked(&t, false, 7, 12));
        assert!(token_blocked(&t, false, 4, 6));
        assert!(token_blocked(&t, false, 20, 21));
        // Adjacent-but-not-overlapping ranges do not block.
        assert!(!token_blocked(&t, false, 10, 20));
        assert!(!token_blocked(&t, false, 0, 5));
        assert!(!token_blocked(&t, false, 30, 40));
        assert!(!token_blocked(&[], false, 0, 10));
    }

    #[test]
    fn go_to_ranges_links_commented_out_import() {
        // Desktop parity: only Name tokens are trivia-filtered, so an
        // import specifier inside a comment still links — while a Name in
        // the same comment stays suppressed.
        let c = state("a.js", "// import { ghost } from './ghost'\nconst y = 1;");
        let text = c.edit.text.clone();
        let spec = text.find("./ghost").unwrap();
        let ranges = c.go_to_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == spec && r.end == spec + "./ghost".len()),
            "commented-out specifier links: {ranges:?}"
        );
        let req = c
            .go_to_token_at(spec + 2)
            .expect("specifier classifies at the caret");
        assert!(matches!(req.target, gtd::GoToTarget::Import(_)));
        let name = text.find("{ ghost }").unwrap() + 2;
        assert!(
            c.go_to_token_at(name).is_none(),
            "imported name inside the comment stays suppressed"
        );
    }

    // -- headless runtime: the modifier overlay's real hit behaviour --------

    use rune::{Cx, Env, Ime, Mods, PhysicalInput, Runtime};

    struct OverlayS {
        code: CodeState,
        hits: Vec<GoToRequest>,
    }

    fn overlay_get(s: &OverlayS) -> &CodeState {
        &s.code
    }
    fn overlay_get_mut(s: &mut OverlayS) -> &mut CodeState {
        &mut s.code
    }
    fn overlay_view(s: &OverlayS, _: &mut Cx<OverlayS>) -> El {
        Code::new(overlay_get, overlay_get_mut)
            .on_go_to(|st: &mut OverlayS, req: GoToRequest, _tx: &mut Tx| st.hits.push(req))
            .build(s, &Ink::new(), &Kit::neutral(true))
    }

    /// Live (non-exiting) link elements: an unmounted overlay's links linger
    /// as `exiting` for Rune's presence animation but take no hits.
    fn link_elements(rt: &Runtime<OverlayS>) -> Vec<rune::runtime::ElementInfo> {
        rt.elements()
            .into_iter()
            .filter(|e| e.path.contains("#link-") && !e.exiting)
            .collect()
    }

    #[test]
    fn conflict_navigation_wraps_and_previous_leaves_the_current_block() {
        let block = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> side\n";
        let mut rt = Runtime::new(
            OverlayS {
                code: CodeState::new("a.rs", format!("prefix\n{block}between\n{block}")),
                hits: Vec::new(),
            },
            overlay_view,
            (900.0, 600.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        let click = |rt: &mut Runtime<OverlayS>, label: &str| {
            let e = rt
                .elements()
                .into_iter()
                .find(|e| e.label.as_deref() == Some(label) && !e.exiting)
                .unwrap();
            let [x, y, w, h] = e.rect;
            rt.pointer_down(x + w / 2.0, y + h / 2.0);
            rt.pointer_up(x + w / 2.0, y + h / 2.0);
            rt.frame(0.0);
        };
        click(&mut rt, "Next conflict");
        let first = rt.state.code.edit.focus;
        click(&mut rt, "Next conflict");
        let second = rt.state.code.edit.focus;
        assert!(second > first);
        click(&mut rt, "Previous conflict");
        assert_eq!(rt.state.code.edit.focus, first);
        click(&mut rt, "Previous conflict");
        assert_eq!(rt.state.code.edit.focus, second);
        click(&mut rt, "Next conflict");
        assert_eq!(rt.state.code.edit.focus, first);
    }

    #[test]
    fn inline_conflict_accept_is_one_undoable_edit_and_rejects_read_only() {
        let original =
            "before\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> side\nafter\n";
        for (choice, expected) in [
            (Choice::Current, "ours\n"),
            (Choice::Incoming, "theirs\n"),
            (Choice::Both, "ours\ntheirs\n"),
        ] {
            let mut rt = Runtime::new(
                OverlayS {
                    code: CodeState::new("a.rs", original),
                    hits: Vec::new(),
                },
                overlay_view,
                (700.0, 500.0),
                Env::new(true, 2.0),
            );
            rt.frame(0.0);
            let key = format!("conflict-7-{choice:?}");
            let button = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(&key))
                .unwrap();
            let [x, y, w, h] = button.rect;
            rt.pointer_move(x + w / 2.0, y + h / 2.0);
            rt.pointer_down(x + w / 2.0, y + h / 2.0);
            rt.pointer_up(x + w / 2.0, y + h / 2.0);
            rt.frame(0.0);
            assert_eq!(
                rt.state.code.edit.text,
                format!("before\n{expected}after\n")
            );
            assert!(rt.state.code.conflicts.is_empty());
            rt.key(KeyEvent {
                key: Key::Command('z'),
                mods: Mods {
                    cmd: true,
                    ..Mods::default()
                },
            });
            rt.frame(0.0);
            assert_eq!(rt.state.code.edit.text, original);
            assert_eq!(rt.state.code.conflicts.len(), 1);
            rt.update(|s, _| s.code.read_only = true);
            rt.frame(0.0);
            let button = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(&key) && !e.exiting)
                .unwrap();
            let [x, y, w, h] = button.rect;
            rt.pointer_down(x + w / 2.0, y + h / 2.0);
            rt.pointer_up(x + w / 2.0, y + h / 2.0);
            rt.frame(0.0);
            assert_eq!(rt.state.code.edit.text, original);
        }
    }

    #[test]
    fn go_to_overlay_is_glyph_accurate_and_modifier_gated() {
        let text = "import { greet } from './greet'\nconst x = greet(1)\n// greet in comment\n";
        let mut rt = Runtime::new(
            OverlayS {
                code: CodeState::new("a.js", text),
                hits: Vec::new(),
            },
            overlay_view,
            (700.0, 500.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        // Modifier up: the overlay is absent — no link elements at all.
        assert!(
            link_elements(&rt).is_empty(),
            "no overlay without the go-to modifier"
        );

        rt.update(|s: &mut OverlayS, _| s.code.go_to_mod = true);
        rt.frame(1.0 / 60.0);
        let links = link_elements(&rt);
        assert!(
            links.len() >= 3,
            "specifier + names mount as links: {links:?}"
        );
        // The comment's 'greet' must not be a link; the specifier and the
        // two code 'greet's must be.
        let spec = text.find("./greet").unwrap();
        let spec_id = format!("{}:{}", spec, spec + "./greet".len());
        let link_rect = links
            .iter()
            .find(|e| e.key.as_deref() == Some(spec_id.as_str()))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("specifier link {spec_id} missing: {links:?}"));
        // A click on the link's glyphs WITHOUT the modifier does nothing.
        let (x, y) = (link_rect[0] + 2.0, link_rect[1] + link_rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.hits.is_empty(), "plain click must not go-to");

        // Hold the platform modifier: the same click reports the request.
        rt.physical_input(PhysicalInput::Modifiers(Mods {
            cmd: true,
            ..Mods::default()
        }));
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.hits.len(), 1, "cmd-click reports one request");
        let req = &rt.state.hits[0];
        assert_eq!((req.from, req.to), (spec, spec + "./greet".len()));
        assert!(
            matches!(req.target, gtd::GoToTarget::Import(_)),
            "{:?}",
            req.target
        );

        // Modifier released: the parent clears go_to_mod and the overlay
        // unmounts on the next frame.
        rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
        rt.update(|s: &mut OverlayS, _| s.code.go_to_mod = false);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        assert!(
            link_elements(&rt).is_empty(),
            "overlay unmounts with the modifier"
        );
        // And a cmd-click where the link was reports nothing: exiting links
        // are inert, so the press falls through to the field below.
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.hits.len(),
            1,
            "no go-to after the overlay unmounts"
        );
    }

    // -- headless runtime: real editing through the mounted field -----------

    struct EditS {
        code: CodeState,
        changed: u32,
    }

    fn edit_get(s: &EditS) -> &CodeState {
        &s.code
    }
    fn edit_get_mut(s: &mut EditS) -> &mut CodeState {
        &mut s.code
    }
    fn edit_view(s: &EditS, _: &mut Cx<EditS>) -> El {
        Code::new(edit_get, edit_get_mut)
            .on_changed(|st: &mut EditS, _tx: &mut Tx| st.changed += 1)
            .build(s, &Ink::new(), &Kit::neutral(true))
    }

    fn edit_runtime(read_only: bool) -> Runtime<EditS> {
        let mut code = CodeState::new("a.txt", "one\ntwo");
        code.read_only = read_only;
        code.edit.focused = true;
        code.edit.set_caret(code.edit.text.len());
        Runtime::new(
            EditS { code, changed: 0 },
            edit_view,
            (800.0, 600.0),
            Env::new(true, 2.0),
        )
    }

    #[test]
    fn edit_surface_fills_blank_space_without_clipping_long_documents() {
        for contents in ["one".to_string(), "line\n".repeat(200)] {
            let mut rt = Runtime::new(
                EditS {
                    code: CodeState::new("a.rs", &contents),
                    changed: 0,
                },
                edit_view,
                (800.0, 600.0),
                Env::new(true, 2.0),
            );
            rt.frame(0.0);
            let field = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(FIELD_KEY))
                .unwrap();
            let viewport = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(SCROLL_KEY))
                .unwrap();
            assert!(
                field.rect[3] >= viewport.rect[3],
                "field covers the viewport: {:?} vs {:?}",
                field.rect,
                viewport.rect
            );
            if contents.len() > 10 {
                assert!(
                    field.rect[3] >= 200.0 * CODE_STYLE.line_px(),
                    "long document keeps content height: {:?}",
                    field.rect
                );
                rt.wheel(viewport.rect[0] + 50.0, viewport.rect[1] + 100.0, -300.0);
                for _ in 0..20 {
                    rt.frame(1.0 / 60.0);
                }
                assert!(
                    rt.state.code.scroll_pos.get()[1] > 0.0,
                    "long text still scrolls"
                );
            } else {
                let x = viewport.rect[0] + 40.0;
                let y = viewport.rect[1] + viewport.rect[3] / 2.0;
                rt.pointer_down(x, y);
                rt.pointer_up(x, y);
                rt.key(KeyEvent {
                    key: Key::Text("!".into()),
                    mods: Mods::default(),
                });
                rt.frame(0.0);
                assert_eq!(rt.state.code.edit.text, "one!");
            }
        }
    }

    #[test]
    fn enter_key_inserts_newline_and_notifies() {
        let mut rt = edit_runtime(false);
        rt.frame(0.0);
        let before = rt.state.code.edit.text.clone();
        let rev = rt.state.code.revision;
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        let text = &rt.state.code.edit.text;
        assert_eq!(
            text.matches('\n').count(),
            before.matches('\n').count() + 1,
            "Enter inserted exactly one newline: {text:?}"
        );
        assert!(rt.state.code.revision > rev, "revision advanced past {rev}");
        assert_eq!(rt.state.changed, 1, "on_changed fired exactly once");
        assert_eq!(rt.state.code.line_count(), 3);
        assert_eq!(rt.state.code.gutter_text(), "1\n2\n3");
        // Undo still owns the inserted newline: Command+Z restores the text.
        rt.key(KeyEvent {
            key: Key::Command('z'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        });
        rt.frame(0.0);
        assert_eq!(rt.state.code.edit.text, before, "undo reverted Enter");
    }

    #[test]
    fn ime_preedit_commit_and_cancel_notify_normally() {
        let mut rt = edit_runtime(false);
        rt.frame(0.0);
        let rev = rt.state.code.revision;
        // Preedit is marked text only: the document is untouched.
        rt.ime(Ime::Preedit("pending".into(), None));
        rt.frame(0.0);
        assert!(rt.state.code.edit.preedit.is_some());
        assert_eq!(rt.state.code.edit.text, "one\ntwo");
        assert_eq!(rt.state.code.revision, rev, "preedit is not an edit");
        // Commit inserts through the normal change path.
        rt.ime(Ime::Commit("仮名".into()));
        rt.frame(0.0);
        assert!(rt.state.code.edit.text.contains("仮名"));
        assert!(rt.state.code.edit.preedit.is_none());
        assert!(rt.state.code.revision > rev, "commit advanced revision");
        assert!(rt.state.changed >= 1, "commit fired on_changed");
        // Cancelling a preedit discards it without touching the text.
        let text = rt.state.code.edit.text.clone();
        rt.ime(Ime::Preedit("discarded".into(), None));
        rt.frame(0.0);
        rt.cancel_composition();
        rt.frame(0.0);
        assert!(rt.state.code.edit.preedit.is_none());
        assert_eq!(rt.state.code.edit.text, text, "cancel kept committed text");
    }

    fn cmd_slash(rt: &mut Runtime<EditS>) {
        rt.key(KeyEvent {
            key: Key::Command('/'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        });
        rt.frame(0.0);
    }

    #[test]
    fn cmd_slash_toggles_a_comment_as_one_undo_step() {
        let mut rt = edit_runtime(false);
        rt.state.code.path = "a.rs".into();
        rt.frame(0.0);
        cmd_slash(&mut rt);
        assert_eq!(rt.state.code.edit.text, "one\n// two");
        assert_eq!(rt.state.changed, 1, "on_changed fired");
        cmd_slash(&mut rt);
        assert_eq!(rt.state.code.edit.text, "one\ntwo", "toggled back off");
        rt.key(KeyEvent {
            key: Key::Command('z'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        });
        rt.frame(0.0);
        assert_eq!(rt.state.code.edit.text, "one\n// two", "undo is one step");
    }

    #[test]
    fn cmd_slash_does_nothing_in_read_only_or_commentless_files() {
        let mut rt = edit_runtime(true);
        rt.state.code.path = "a.rs".into();
        rt.frame(0.0);
        cmd_slash(&mut rt);
        assert_eq!(rt.state.code.edit.text, "one\ntwo", "read-only");
        let mut rt = edit_runtime(false);
        rt.state.code.path = "a.json".into();
        rt.frame(0.0);
        cmd_slash(&mut rt);
        assert_eq!(rt.state.code.edit.text, "one\ntwo", "JSON has no comments");
    }

    #[test]
    fn read_only_field_refuses_keyboard_mutation() {
        let mut rt = edit_runtime(true);
        rt.frame(0.0);
        let rev = rt.state.code.revision;
        for key in [Key::Text("x".into()), Key::Enter, Key::Backspace] {
            rt.key(KeyEvent {
                key,
                mods: Mods::default(),
            });
            rt.frame(0.0);
        }
        assert_eq!(rt.state.code.edit.text, "one\ntwo");
        assert_eq!(rt.state.code.revision, rev);
        assert_eq!(rt.state.changed, 0, "read-only fires no change callback");
    }
}

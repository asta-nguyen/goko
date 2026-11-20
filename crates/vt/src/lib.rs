//! A terminal emulator for the app and the daemon: libghostty-vt behind a safe
//! `Vt`.
//!
//! `Vt` owns one terminal: feed it the program's bytes with [`Vt::write`], read
//! what it wants to say back with [`Vt::take_replies`], and read the screen as
//! a [`Frame`]. A synchronized update (DEC 2026) holds the frame at the state
//! the program last finished, for at most [`MAX_HOLD`].

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
use ffi::*;

/// How long a synchronized update may hold the frame back.
pub const MAX_HOLD: Duration = Duration::from_millis(1000);
/// The most a program may put on the clipboard in one OSC 52 write.
const MAX_CLIPBOARD_BYTES: usize = 1 << 20;
const SYNC_OUTPUT: GhosttyMode = 2026;
/// On from the start, as in the reference terminal: VS16 makes a glyph two
/// cells wide and in color.
const GRAPHEME_CLUSTER: GhosttyMode = 2027;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<GhosttyColorRgb> for Rgb {
    fn from(c: GhosttyColorRgb) -> Self {
        Rgb(c.r, c.g, c.b)
    }
}

/// The colors a terminal draws with: the default text and background, the
/// cursor, the selection's background and the 16 ANSI colors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub palette16: [Rgb; 16],
}

impl Default for Theme {
    fn default() -> Self {
        let c = |v: u32| Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        Theme {
            fg: c(0xd4d4d4),
            bg: c(0x1e1e1e),
            cursor: c(0xffffff),
            selection_bg: c(0x264f78),
            palette16: [
                c(0x000000),
                c(0xcd3131),
                c(0x0dbc79),
                c(0xe5e510),
                c(0x2472c8),
                c(0xbc3fbc),
                c(0x11a8cd),
                c(0xe5e5e5),
                c(0x666666),
                c(0xf14c4c),
                c(0x23d18b),
                c(0xf5f543),
                c(0x3b8eea),
                c(0xd670d6),
                c(0x29b8db),
                c(0xffffff),
            ],
        }
    }
}

/// How a cell is underlined (SGR 4, 4:1 to 4:5, 21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Cell {
    /// Empty for blank cells and for the tail half of a wide character.
    pub text: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: Option<Underline>,
    /// The underline's own color (SGR 58); `None` draws it in the text's color.
    pub underline_color: Option<Rgb>,
    pub strike: bool,
    pub wide: bool,
    /// The cell is only room a wide character takes: the half after its head,
    /// or the last cell of a row it left when the head moved to the next.
    /// It carries no text of its own.
    pub spacer: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
    pub color: Rgb,
}

pub struct Frame {
    /// A row that did not change since the last frame is the same `Arc`.
    pub rows: Vec<Arc<Vec<Cell>>>,
    /// Every row changed (output scrolled, a resize, new colors): none is kept.
    pub all: bool,
    /// The cursor when the program shows it and it is in the viewport.
    pub cursor: Option<Cursor>,
    /// The cursor's cell even when the program hides it.
    pub cursor_cell: Option<(u16, u16)>,
    pub bg: Rgb,
    /// Per row: the terminal wrapped it onto the next.
    pub wraps: Vec<bool>,
}

/// The scrollback's bounds: whichever is reached first drops the oldest rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub lines: usize,
    pub bytes: usize,
}

/// What a selection covers around the cell it starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKind {
    Cells,
    Word,
    Line,
}

/// A word or line's first and last cell, (column, screen row).
type Span = ((u16, usize), (u16, usize));

/// One cell of a row's text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextCell {
    pub col: u16,
    pub text: String,
    /// The character takes this cell and the next.
    pub wide: bool,
}

/// A screen row's text, cell by cell.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub cells: Vec<TextCell>,
    /// The terminal wrapped the row onto the next.
    pub wraps: bool,
}

/// Holds what the terminal's callbacks write into; boxed so its address is stable.
struct Shared {
    state: GhosttyRenderState,
    size: GhosttySizeReportSize,
    held_since: Option<Instant>,
    title_changed: bool,
    /// What the terminal writes back to the program.
    replies: Vec<u8>,
    clipboard: Option<String>,
}

fn ok(r: GhosttyResult, what: &str) {
    assert!(r == GHOSTTY_SUCCESS, "{what} failed: {r}");
}

unsafe extern "C" fn on_render_hold(t: GhosttyTerminal, userdata: *mut c_void, held: bool) {
    // SAFETY: `userdata` is the `Shared` box `Vt::new` registered; the terminal
    // calls back only while a `Vt` method holds it, and `Vt` owns both.
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if held {
        // Capture the finished frame the program wants on screen before it redraws.
        unsafe { ghostty_render_state_update(shared.state, t) };
        shared.held_since = Some(Instant::now());
    } else {
        shared.held_since = None;
    }
}

unsafe extern "C" fn on_size(
    _t: GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut GhosttySizeReportSize,
) -> bool {
    // SAFETY: as in `on_render_hold`.
    let shared = unsafe { &*(userdata as *const Shared) };
    if shared.size.cell_width == 0 || shared.size.cell_height == 0 {
        return false;
    }
    unsafe { *out = shared.size };
    true
}

unsafe extern "C" fn on_title(_t: GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: as in `on_render_hold`.
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    shared.title_changed = true;
}

unsafe extern "C" fn on_write_pty(
    _t: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    // SAFETY: as in `on_render_hold`; `data` is valid for `len` bytes.
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if !data.is_null() && len > 0 {
        shared
            .replies
            .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    }
}

unsafe extern "C" fn on_clipboard_write(
    _t: GhosttyTerminal,
    userdata: *mut c_void,
    request: *const GhosttyClipboardWrite,
) {
    // SAFETY: as in `on_render_hold`; `request` is valid for the call.
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    let request = unsafe { &*request };
    if request.size < std::mem::size_of::<GhosttyClipboardWrite>()
        || request.location != GHOSTTY_CLIPBOARD_LOCATION_STANDARD
    {
        return;
    }
    let text = if request.contents_len == 0 {
        ""
    } else {
        if request.contents_len != 1 || request.contents.is_null() {
            return;
        }
        let content = unsafe { &*request.contents };
        if content.mime.ptr.is_null() || content.data.len > MAX_CLIPBOARD_BYTES {
            return;
        }
        let mime = unsafe { std::slice::from_raw_parts(content.mime.ptr, content.mime.len) };
        if mime != b"text/plain" {
            return;
        }
        let bytes = if content.data.len == 0 {
            &[][..]
        } else if content.data.ptr.is_null() {
            return;
        } else {
            unsafe { std::slice::from_raw_parts(content.data.ptr, content.data.len) }
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        text
    };
    // Only the last write of a batch is kept.
    shared.clipboard = Some(text.to_owned());
}

/// What of the cursor a frame shows: (on screen, x, y, visible, style).
type CursorKey = (bool, u16, u16, bool, i32);

/// A row as last read, with the columns of it that were selected.
type SeenRow = (Arc<Vec<Cell>>, Option<(u16, u16)>);

pub struct Vt {
    term: GhosttyTerminal,
    shared: Box<Shared>,
    rows_it: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    pub cols: u16,
    pub rows: u16,
    theme: Theme,
    /// The built-in 256-color palette, which the theme's 16 colors override.
    base_palette: [GhosttyColorRgb; 256],
    /// The cursor and the colors as last drawn: changes to them do not mark the
    /// render state dirty.
    drawn: (CursorKey, u64),
    /// The rows read last frame by libghostty's row id, with their selection:
    /// one that is not dirty is the same row and is not read again.
    seen: HashMap<[u64; 2], SeenRow>,
    /// The width and colors they were read with: a change reads every row.
    seen_with: (u16, u64),
}

// SAFETY: a libghostty terminal has no thread affinity: it is plain memory
// reached only through `&mut self`/`&self` here, and its callbacks write into
// the `Shared` box this `Vt` owns. `Vt` is not `Sync`.
unsafe impl Send for Vt {}

impl Vt {
    pub fn new(cols: u16, rows: u16, limits: Limits) -> Vt {
        let (cols, rows) = (cols.max(1), rows.max(1));
        unsafe {
            let mut term = std::ptr::null_mut();
            ok(
                ghostty_terminal_new(std::ptr::null(), &mut term, cols, rows),
                "terminal_new",
            );
            let cluster = GhosttyTerminalModeConfig {
                mode: GRAPHEME_CLUSTER,
                value: true,
            };
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                &cluster as *const _ as *const c_void,
            );
            let mut state = std::ptr::null_mut();
            ok(
                ghostty_render_state_new(std::ptr::null(), &mut state),
                "render_state_new",
            );
            let mut rows_it = std::ptr::null_mut();
            ok(
                ghostty_render_state_row_iterator_new(std::ptr::null(), &mut rows_it),
                "row_iterator_new",
            );
            let mut cells = std::ptr::null_mut();
            ok(
                ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells),
                "row_cells_new",
            );

            let mut shared = Box::new(Shared {
                state,
                size: GhosttySizeReportSize {
                    rows,
                    columns: cols,
                    cell_width: 1,
                    cell_height: 1,
                },
                held_since: None,
                title_changed: false,
                replies: Vec::new(),
                clipboard: None,
            });
            let ud = &mut *shared as *mut Shared as *const c_void;
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_USERDATA, ud);
            let hold: GhosttyTerminalRenderHoldFn = Some(on_render_hold);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
                std::mem::transmute::<GhosttyTerminalRenderHoldFn, *const c_void>(hold),
            );
            let title: GhosttyTerminalTitleChangedFn = Some(on_title);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                std::mem::transmute::<GhosttyTerminalTitleChangedFn, *const c_void>(title),
            );
            let size: GhosttyTerminalSizeFn = Some(on_size);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SIZE,
                std::mem::transmute::<GhosttyTerminalSizeFn, *const c_void>(size),
            );
            let write: GhosttyTerminalWritePtyFn = Some(on_write_pty);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                std::mem::transmute::<GhosttyTerminalWritePtyFn, *const c_void>(write),
            );
            let clipboard: GhosttyTerminalClipboardWriteFn = Some(on_clipboard_write);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                std::mem::transmute::<GhosttyTerminalClipboardWriteFn, *const c_void>(clipboard),
            );
            let max_clipboard: usize = MAX_CLIPBOARD_BYTES;
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES,
                &max_clipboard as *const _ as *const c_void,
            );
            // No read callback: a program can never read the local clipboard.

            ghostty_render_state_update(state, term);
            let mut colors = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            );

            ok(
                ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                    &limits.bytes as *const _ as *const c_void,
                ),
                "scrollback_max_bytes",
            );
            ok(
                ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                    &limits.lines as *const _ as *const c_void,
                ),
                "scrollback_max_lines",
            );

            let mut vt = Vt {
                term,
                shared,
                rows_it,
                cells,
                cols,
                rows,
                theme: Theme::default(),
                base_palette: colors.palette,
                drawn: Default::default(),
                seen: HashMap::new(),
                seen_with: (0, 0),
            };
            vt.apply_theme();
            vt
        }
    }

    /// Switches colors; the next frame reads every row again.
    pub fn set_theme(&mut self, theme: Theme) {
        if self.theme != theme {
            self.theme = theme;
            self.apply_theme();
            self.seen.clear();
        }
    }

    fn apply_theme(&mut self) {
        let c = |rgb: Rgb| GhosttyColorRgb {
            r: rgb.0,
            g: rgb.1,
            b: rgb.2,
        };
        let t = &self.theme;
        let (fg, bg, cur) = (c(t.fg), c(t.bg), c(t.cursor));
        let mut palette = self.base_palette;
        for (i, rgb) in t.palette16.iter().enumerate() {
            palette[i] = c(*rgb);
        }
        unsafe {
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                &fg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                &bg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                &cur as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                &palette as *const _ as *const c_void,
            );
        }
    }

    /// Parse the program's output.
    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: `term` is live and `bytes` is valid for its length.
        unsafe { ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len()) };
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.shared.size = GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: 1,
            cell_height: 1,
        };
        unsafe { ghostty_terminal_resize(self.term, cols, rows, 1, 1) };
    }

    /// What the terminal wants to say back to the program since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }

    /// The text the program last put on the clipboard (OSC 52) since the last call.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.shared.clipboard.take()
    }

    /// The title if the program set it since the last call (empty when reset).
    pub fn take_title(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.shared.title_changed) {
            return None;
        }
        let mut s = GhosttyString::default();
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_TITLE,
                &mut s as *mut _ as *mut c_void,
            )
        };
        if s.ptr.is_null() {
            return Some(String::new());
        }
        // SAFETY: `ptr` is valid for `len` bytes until the next terminal call.
        let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn get_bool(&self, data: GhosttyTerminalData) -> bool {
        let mut v = false;
        unsafe { ghostty_terminal_get(self.term, data, &mut v as *mut _ as *mut c_void) };
        v
    }

    fn get_u16(&self, data: GhosttyTerminalData) -> u16 {
        let mut v = 0;
        unsafe { ghostty_terminal_get(self.term, data, &mut v as *mut _ as *mut c_void) };
        v
    }

    /// The cursor's cell on the visible screen, (column, row): `None` when the
    /// view is scrolled up so the cursor is below the viewport's bottom edge.
    /// The cursor is reported even when the program hides it.
    pub fn cursor_cell(&self) -> Option<(u16, u16)> {
        let x = self.get_u16(GHOSTTY_TERMINAL_DATA_CURSOR_X);
        let y = self.get_u16(GHOSTTY_TERMINAL_DATA_CURSOR_Y);
        let row = y as usize + self.display_offset();
        (x < self.cols && row < self.rows as usize).then_some((x, row as u16))
    }

    /// Whether a DEC private mode (`CSI ? <n> h`) is set.
    pub fn mode(&self, mode: u16) -> bool {
        let mut cfg = GhosttyTerminalModeConfig { mode, value: false };
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MODE,
                &mut cfg as *mut _ as *mut c_void,
            )
        };
        cfg.value
    }

    /// Whether the program asked for any mouse reports (1000, 1002 or 1003).
    pub fn mouse_tracking(&self) -> bool {
        self.get_bool(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
    }

    pub fn alt_screen(&self) -> bool {
        let mut screen: GhosttyTerminalScreen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                &mut screen as *mut _ as *mut c_void,
            )
        };
        screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    fn scrollbar(&self) -> GhosttyTerminalScrollbar {
        let mut sb = GhosttyTerminalScrollbar::default();
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                &mut sb as *mut _ as *mut c_void,
            )
        };
        sb
    }

    /// The rows above the screen the scrollback holds.
    pub fn history_size(&self) -> usize {
        let sb = self.scrollbar();
        sb.total.saturating_sub(sb.len) as usize
    }

    /// How many rows the viewport is scrolled up from the bottom.
    pub fn display_offset(&self) -> usize {
        let sb = self.scrollbar();
        sb.total.saturating_sub(sb.len).saturating_sub(sb.offset) as usize
    }

    /// Scrolls the viewport; positive reveals older rows.
    pub fn scroll_viewport(&mut self, delta: isize) {
        let mut s = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_DELTA,
            ..Default::default()
        };
        // Ghostty's delta is positive downwards.
        s.value.delta = -delta;
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
    }

    pub fn scroll_to_bottom(&mut self) {
        let s = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            ..Default::default()
        };
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
    }

    fn screen_ref(&self, x: u16, y: usize) -> Option<GhosttyGridRef> {
        let mut r = GhosttyGridRef {
            size: std::mem::size_of::<GhosttyGridRef>(),
            ..Default::default()
        };
        let mut p = GhosttyPoint {
            tag: GHOSTTY_POINT_TAG_SCREEN,
            ..Default::default()
        };
        p.value.coordinate = GhosttyPointCoordinate { x, y: y as u32 };
        (unsafe { ghostty_terminal_grid_ref(self.term, p, &mut r) } == GHOSTTY_SUCCESS).then_some(r)
    }

    /// The (column, screen row) a grid reference points at.
    fn position(&self, r: &GhosttyGridRef) -> Option<(u16, usize)> {
        let mut p = GhosttyPointCoordinate::default();
        let result = unsafe {
            ghostty_terminal_point_from_grid_ref(self.term, r, GHOSTTY_POINT_TAG_SCREEN, &mut p)
        };
        (result == GHOSTTY_SUCCESS).then_some((p.x, p.y as usize))
    }

    /// The word or line around a cell, as its first and last cell.
    fn span_at(&self, kind: SelectKind, at: (u16, usize)) -> Option<Span> {
        let r = self.screen_ref(at.0, at.1)?;
        let mut selection = GhosttySelection {
            size: std::mem::size_of::<GhosttySelection>(),
            start: r,
            end: r,
            ..Default::default()
        };
        let result = unsafe {
            match kind {
                SelectKind::Cells => return Some((at, at)),
                SelectKind::Word => {
                    let options = GhosttyTerminalSelectWordOptions {
                        size: std::mem::size_of::<GhosttyTerminalSelectWordOptions>(),
                        ref_: r,
                        ..Default::default()
                    };
                    ghostty_terminal_select_word(self.term, &options, &mut selection)
                }
                SelectKind::Line => {
                    let options = GhosttyTerminalSelectLineOptions {
                        size: std::mem::size_of::<GhosttyTerminalSelectLineOptions>(),
                        ref_: r,
                        semantic_prompt_boundary: true,
                        ..Default::default()
                    };
                    ghostty_terminal_select_line(self.term, &options, &mut selection)
                }
            }
        };
        if result != GHOSTTY_SUCCESS {
            return None;
        }
        Some((
            self.position(&selection.start)?,
            self.position(&selection.end)?,
        ))
    }

    /// Selects from the cell `anchor` to the cell `to`, both (column, screen
    /// row): the cells between them, or for a word or a line everything from
    /// the first to the last word or line the two cells touch (so a drag
    /// after a double click grows by words). A cell with no word under it
    /// counts as itself. The selection follows the text as output scrolls.
    pub fn select(&mut self, kind: SelectKind, anchor: (u16, usize), to: (u16, usize)) {
        let key = |p: (u16, usize)| (p.1, p.0);
        let spans = [
            self.span_at(kind, anchor).unwrap_or((anchor, anchor)),
            self.span_at(kind, to).unwrap_or((to, to)),
        ];
        let first = spans
            .iter()
            .flat_map(|s| [s.0, s.1])
            .min_by_key(|p| key(*p));
        let last = spans
            .iter()
            .flat_map(|s| [s.0, s.1])
            .max_by_key(|p| key(*p));
        let (Some(first), Some(last)) = (first, last) else {
            return;
        };
        let (Some(start), Some(end)) = (
            self.screen_ref(first.0, first.1),
            self.screen_ref(last.0, last.1),
        ) else {
            self.clear_selection();
            return;
        };
        let selection = GhosttySelection {
            size: std::mem::size_of::<GhosttySelection>(),
            start,
            end,
            ..Default::default()
        };
        unsafe {
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_SELECTION,
                &selection as *const _ as *const c_void,
            );
        }
    }

    /// How many rows the scrollback and the screen hold together.
    pub fn screen_rows(&self) -> usize {
        self.scrollbar().total as usize
    }

    /// The screen row the viewport starts at.
    pub fn viewport_top(&self) -> usize {
        self.scrollbar().offset as usize
    }

    /// Whether the terminal wrapped screen row `y` onto the next.
    fn row_wraps(&self, y: usize) -> bool {
        let Some(r) = self.screen_ref(0, y) else {
            return false;
        };
        let mut line: GhosttyRow = Default::default();
        let mut wrap = false;
        // SAFETY: `r` is a fresh reference; `wrap` is a bool the call fills.
        unsafe {
            ghostty_grid_ref_row(&r, &mut line) == GHOSTTY_SUCCESS
                && ghostty_row_get(
                    line,
                    GHOSTTY_ROW_DATA_WRAP,
                    &mut wrap as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS
                && wrap
        }
    }

    /// The text of screen row `y` (0 is the oldest row of the scrollback), a
    /// cell at a time, for searching. A blank cell reads as a space; the
    /// second half of a wide character has no cell of its own.
    pub fn text_row(&self, y: usize) -> Option<TextRow> {
        let mut r = self.screen_ref(0, y)?;
        let mut cells = Vec::with_capacity(self.cols as usize);
        for x in 0..self.cols {
            r.x = x;
            // SAFETY: `r` points into the terminal; the buffers are the sizes passed.
            let wide = unsafe {
                let mut cell: GhosttyCell = Default::default();
                ghostty_grid_ref_cell(&r, &mut cell);
                let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
                ghostty_cell_get(
                    cell,
                    GHOSTTY_CELL_DATA_WIDE,
                    &mut wide as *mut _ as *mut c_void,
                );
                wide
            };
            if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD {
                continue;
            }
            let mut cps = [0u32; 16];
            let mut len = 0usize;
            unsafe { ghostty_grid_ref_graphemes(&r, cps.as_mut_ptr(), cps.len(), &mut len) };
            let mut text: String = cps[..len.min(cps.len())]
                .iter()
                .filter_map(|c| char::from_u32(*c))
                .collect();
            if text.is_empty() {
                text.push(' ');
            }
            cells.push(TextCell {
                col: x,
                text,
                wide: wide == GHOSTTY_CELL_WIDE_WIDE,
            });
        }
        Some(TextRow {
            cells,
            wraps: self.row_wraps(y),
        })
    }

    pub fn clear_selection(&mut self) {
        unsafe {
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, std::ptr::null())
        };
    }

    /// The selected text, if anything is selected.
    pub fn selection_text(&self) -> Option<String> {
        unsafe {
            let opts = GhosttyTerminalSelectionFormatOptions {
                size: std::mem::size_of::<GhosttyTerminalSelectionFormatOptions>(),
                emit: GHOSTTY_FORMATTER_FORMAT_PLAIN,
                unwrap: true,
                trim: true,
                ..Default::default()
            };
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0usize;
            if ghostty_terminal_selection_format_alloc(
                self.term,
                std::ptr::null(),
                opts,
                &mut ptr,
                &mut len,
            ) != GHOSTTY_SUCCESS
                || ptr.is_null()
            {
                return None;
            }
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ghostty_free(std::ptr::null(), ptr, len);
            (!text.is_empty()).then_some(text)
        }
    }

    /// VT bytes that rebuild this terminal in a fresh one of the same size: the
    /// retained scrollback, the screen, the cursor and its style, the modes, the
    /// palette, the scrolling region and the tab stops.
    pub fn dump_vt(&mut self) -> Vec<u8> {
        // SAFETY: the formatter borrows `term`, is used once and freed before
        // returning; the allocated buffer is copied and freed with the same
        // (default) allocator.
        unsafe {
            let screen = GhosttyFormatterScreenExtra {
                size: std::mem::size_of::<GhosttyFormatterScreenExtra>(),
                cursor: true,
                style: true,
                hyperlink: true,
                protection: true,
                kitty_keyboard: true,
                charsets: true,
            };
            let extra = GhosttyFormatterTerminalExtra {
                size: std::mem::size_of::<GhosttyFormatterTerminalExtra>(),
                palette: true,
                modes: true,
                scrolling_region: true,
                tabstops: true,
                pwd: true,
                keyboard: true,
                screen,
            };
            let options = GhosttyFormatterTerminalOptions {
                size: std::mem::size_of::<GhosttyFormatterTerminalOptions>(),
                emit: GHOSTTY_FORMATTER_FORMAT_VT,
                unwrap: true,
                trim: false,
                extra,
                selection: std::ptr::null(),
            };
            let mut formatter = std::ptr::null_mut();
            ok(
                ghostty_formatter_terminal_new(
                    std::ptr::null(),
                    &mut formatter,
                    self.term,
                    options,
                ),
                "formatter_new",
            );
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0usize;
            let result =
                ghostty_formatter_format_alloc(formatter, std::ptr::null(), &mut ptr, &mut len);
            ghostty_formatter_free(formatter);
            ok(result, "formatter_format");
            let mut out = if ptr.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(ptr, len).to_vec()
            };
            ghostty_free(std::ptr::null(), ptr, len);
            // The dump stops after the last row with text, so a replay that has
            // scrolled would show the content too low by the blank rows under
            // it (the cursor often sits on one). Scroll the history those rows
            // stand for off the screen from its bottom row, then put the cursor
            // where it was (the line feeds moved it).
            let scrolls = self.blank_tail().min(self.history_size());
            if scrolls > 0 {
                out.extend(format!("\x1b[{};1H", self.rows).as_bytes());
                out.extend(std::iter::repeat_n(b'\n', scrolls));
                let mut cur = GhosttyRenderStateCursor {
                    size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                    ..Default::default()
                };
                ghostty_render_state_get(
                    self.shared.state,
                    GHOSTTY_RENDER_STATE_DATA_CURSOR,
                    &mut cur as *mut _ as *mut c_void,
                );
                if cur.viewport_has_value {
                    out.extend(
                        format!("\x1b[{};{}H", cur.viewport_y + 1, cur.viewport_x + 1).as_bytes(),
                    );
                }
            }
            out
        }
    }

    /// How many rows at the bottom of the viewport hold no text (a row that
    /// only has a background counts as blank, as it does in the dump).
    fn blank_tail(&mut self) -> usize {
        // SAFETY: the iterators belong to this `Vt` and are only used here and
        // in `frame`, never at the same time.
        unsafe {
            let state = self.shared.state;
            ghostty_render_state_update(state, self.term);
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.rows_it as *mut _ as *mut c_void,
            );
            let (mut rows, mut filled) = (0usize, 0usize);
            while ghostty_render_state_row_iterator_next(self.rows_it) {
                ghostty_render_state_row_get(
                    self.rows_it,
                    GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    &mut self.cells as *mut _ as *mut c_void,
                );
                while ghostty_render_state_row_cells_next(self.cells) {
                    let mut len: u32 = 0;
                    ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                        &mut len as *mut _ as *mut c_void,
                    );
                    if len > 0 {
                        filled = rows + 1;
                        break;
                    }
                }
                rows += 1;
            }
            rows - filled
        }
    }

    /// The cursor and the colors (from the updated render state), as a key.
    unsafe fn drawable(&self) -> (CursorKey, u64) {
        unsafe {
            let mut cur = GhosttyRenderStateCursor {
                size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                self.shared.state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cur as *mut _ as *mut c_void,
            );
            let cursor = (
                cur.viewport_has_value,
                cur.viewport_x,
                cur.viewport_y,
                cur.visible,
                cur.visual_style as i32,
            );
            let mut c = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                self.shared.state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut c as *mut _ as *mut c_void,
            );
            let mut h = std::hash::DefaultHasher::new();
            let rgb = |h: &mut std::hash::DefaultHasher, x: GhosttyColorRgb| {
                std::hash::Hash::hash(&(x.r, x.g, x.b), h)
            };
            for x in [c.background, c.foreground]
                .into_iter()
                .chain(c.palette)
                .chain(c.cursor_has_value.then_some(c.cursor))
            {
                rgb(&mut h, x);
            }
            (cursor, std::hash::Hasher::finish(&h))
        }
    }

    /// Whether output since the last frame changed what is on screen: a title,
    /// or a cursor parked where it was, needs no frame.
    pub fn shows_change(&mut self) -> bool {
        if self.shared.held_since.is_some() {
            return true;
        }
        unsafe {
            let state = self.shared.state;
            ghostty_render_state_update(state, self.term);
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_DIRTY,
                &mut dirty as *mut _ as *mut c_void,
            );
            dirty != GHOSTTY_RENDER_STATE_DIRTY_FALSE || self.drawable() != self.drawn
        }
    }

    /// When a synchronized update that is holding the frame stops holding it.
    pub fn render_hold_deadline(&self) -> Option<Instant> {
        self.shared.held_since.map(|since| since + MAX_HOLD)
    }

    /// Updates the render state (respecting synchronized output) and reads one frame.
    pub fn frame(&mut self) -> Frame {
        unsafe {
            if self
                .shared
                .held_since
                .is_some_and(|since| since.elapsed() > MAX_HOLD)
            {
                let cfg = GhosttyTerminalModeConfig {
                    mode: SYNC_OUTPUT,
                    value: false,
                };
                ghostty_terminal_set(
                    self.term,
                    GHOSTTY_TERMINAL_OPT_MODE,
                    &cfg as *const _ as *const c_void,
                );
                self.shared.held_since = None;
            }
            let state = self.shared.state;
            if self.shared.held_since.is_none() {
                ghostty_render_state_update(state, self.term);
            }

            let mut colors = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            );
            let fg_default = Rgb::from(colors.foreground);
            let bg_default = Rgb::from(colors.background);

            // Rows libghostty keeps clean are taken as read last frame; a
            // resize or new colors (which do not mark rows dirty) read them all.
            let with = (self.cols, self.drawable().1);
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_DIRTY,
                &mut dirty as *mut _ as *mut c_void,
            );
            let all = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL || with != self.seen_with;
            let mut seen = HashMap::with_capacity(self.rows as usize);
            let mut rows = Vec::with_capacity(self.rows as usize);
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.rows_it as *mut _ as *mut c_void,
            );
            while ghostty_render_state_row_iterator_next(self.rows_it) {
                let mut sel = GhosttyRenderStateRowSelection {
                    size: std::mem::size_of::<GhosttyRenderStateRowSelection>(),
                    ..Default::default()
                };
                let selected = ghostty_render_state_row_get(
                    self.rows_it,
                    GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    &mut sel as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS;
                let selection = selected.then_some((sel.start_x, sel.end_x));
                let mut id = GhosttyRenderStateRowId::default();
                let known = ghostty_render_state_row_get(
                    self.rows_it,
                    GHOSTTY_RENDER_STATE_ROW_DATA_ID,
                    &mut id as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS;
                // Everything changed: no row is kept (but each is remembered).
                let mut row_dirty = true;
                if !all {
                    ghostty_render_state_row_get(
                        self.rows_it,
                        GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                        &mut row_dirty as *mut _ as *mut c_void,
                    );
                }
                let same = (known && !row_dirty)
                    .then(|| self.seen.get(&id.bits))
                    .flatten()
                    .filter(|(_, s)| *s == selection);
                let row = match same {
                    Some((row, _)) => row.clone(),
                    None => {
                        ghostty_render_state_row_get(
                            self.rows_it,
                            GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                            &mut self.cells as *mut _ as *mut c_void,
                        );
                        let mut row = Vec::with_capacity(self.cols as usize);
                        let mut x: u16 = 0;
                        while ghostty_render_state_row_cells_next(self.cells) {
                            row.push(self.read_cell(
                                fg_default,
                                &colors.palette,
                                selected && x >= sel.start_x && x <= sel.end_x,
                            ));
                            x += 1;
                        }
                        Arc::new(row)
                    }
                };
                if known {
                    seen.insert(id.bits, (row.clone(), selection));
                }
                rows.push(row);
            }
            (self.seen, self.seen_with) = (seen, with);

            let mut cur = GhosttyRenderStateCursor {
                size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cur as *mut _ as *mut c_void,
            );
            let cursor = (cur.visible && cur.viewport_has_value).then(|| Cursor {
                x: if cur.wide_tail {
                    cur.viewport_x.saturating_sub(1)
                } else {
                    cur.viewport_x
                },
                y: cur.viewport_y,
                shape: match cur.visual_style {
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Bar,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => CursorShape::Hollow,
                    _ => CursorShape::Block,
                },
                color: if colors.cursor_has_value {
                    colors.cursor.into()
                } else {
                    fg_default
                },
            });

            self.drawn = self.drawable();
            ghostty_render_state_clean(state);
            let cursor_cell = cur
                .viewport_has_value
                .then_some((cur.viewport_x, cur.viewport_y));
            let top = self.viewport_top();
            let wraps = (0..rows.len()).map(|y| self.row_wraps(top + y)).collect();
            Frame {
                rows,
                all,
                cursor,
                cursor_cell,
                bg: bg_default,
                wraps,
            }
        }
    }

    unsafe fn read_cell(
        &mut self,
        fg_default: Rgb,
        palette: &[GhosttyColorRgb; 256],
        selected: bool,
    ) -> Cell {
        unsafe {
            let c = self.cells;
            let mut raw: GhosttyCell = Default::default();
            ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                &mut raw as *mut _ as *mut c_void,
            );
            let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
            ghostty_cell_get(
                raw,
                GHOSTTY_CELL_DATA_WIDE,
                &mut wide as *mut _ as *mut c_void,
            );

            let mut style = GhosttyStyle {
                size: std::mem::size_of::<GhosttyStyle>(),
                ..Default::default()
            };
            ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut style as *mut _ as *mut c_void,
            );

            let mut fg = GhosttyColorRgb::default();
            let fg = if ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                &mut fg as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS
            {
                Rgb::from(fg)
            } else {
                fg_default
            };
            let mut bg = GhosttyColorRgb::default();
            let bg = (ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                &mut bg as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS)
                .then(|| Rgb::from(bg));

            let (mut fg, mut bg) = (fg, bg);
            if style.inverse {
                let new_fg = bg.unwrap_or(self.theme.bg);
                bg = Some(fg);
                fg = new_fg;
            }
            if selected {
                bg = Some(self.theme.selection_bg);
            }
            let mut text = String::new();
            if wide != GHOSTTY_CELL_WIDE_SPACER_TAIL
                && wide != GHOSTTY_CELL_WIDE_SPACER_HEAD
                && !style.invisible
            {
                let mut len: u32 = 0;
                ghostty_render_state_row_cells_get(
                    c,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    &mut len as *mut _ as *mut c_void,
                );
                if len > 0 {
                    let mut cps = vec![0u32; len as usize];
                    ghostty_render_state_row_cells_get(
                        c,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                        cps.as_mut_ptr() as *mut c_void,
                    );
                    text = cps.into_iter().filter_map(char::from_u32).collect();
                }
            }

            let underline = match style.underline as GhosttySgrUnderline {
                GHOSTTY_SGR_UNDERLINE_NONE => None,
                GHOSTTY_SGR_UNDERLINE_DOUBLE => Some(Underline::Double),
                GHOSTTY_SGR_UNDERLINE_CURLY => Some(Underline::Curly),
                GHOSTTY_SGR_UNDERLINE_DOTTED => Some(Underline::Dotted),
                GHOSTTY_SGR_UNDERLINE_DASHED => Some(Underline::Dashed),
                _ => Some(Underline::Single),
            };
            // Reverse video swaps the text's colors, not the underline's own.
            let underline_color = underline.and_then(|_| match style.underline_color.tag {
                GHOSTTY_STYLE_COLOR_PALETTE => Some(Rgb::from(
                    palette[style.underline_color.value.palette as usize],
                )),
                GHOSTTY_STYLE_COLOR_RGB => Some(Rgb::from(style.underline_color.value.rgb)),
                _ => None,
            });

            Cell {
                text,
                fg,
                bg,
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                underline,
                underline_color,
                strike: style.strikethrough,
                wide: wide == GHOSTTY_CELL_WIDE_WIDE,
                spacer: wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                    || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD,
            }
        }
    }
}

impl Drop for Vt {
    fn drop(&mut self) {
        // SAFETY: each handle was created in `Vt::new` and is freed once.
        unsafe {
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows_it);
            ghostty_terminal_free(self.term);
            ghostty_render_state_free(self.shared.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        lines: 5_000,
        bytes: 64 << 20,
    };

    /// A row's text with the blank cells' trailing spaces dropped.
    fn text(row: &[Cell]) -> String {
        row.iter()
            .map(|c| {
                if c.text.is_empty() {
                    " "
                } else {
                    c.text.as_str()
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn red_text_has_the_themes_red_foreground() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"\x1b[31mhi\x1b[0m");
        let frame = vt.frame();
        assert_eq!(text(&frame.rows[0]), "hi");
        assert_eq!(frame.rows[0][0].fg, Theme::default().palette16[1]);
        assert_eq!(frame.rows[0][1].fg, Theme::default().palette16[1]);
        assert_eq!(frame.rows[0][2].fg, Theme::default().fg);
    }

    #[test]
    fn a_wrapped_line_is_one_row_after_the_terminal_widens() {
        let mut vt = Vt::new(10, 4, LIMITS);
        vt.write(b"abcdefghijklmno");
        let frame = vt.frame();
        assert_eq!(text(&frame.rows[0]), "abcdefghij");
        assert_eq!(text(&frame.rows[1]), "klmno");
        vt.resize(20, 4);
        let frame = vt.frame();
        assert_eq!(text(&frame.rows[0]), "abcdefghijklmno");
        assert_eq!(text(&frame.rows[1]), "");
    }

    #[test]
    fn a_device_attributes_query_is_answered() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"\x1b[c");
        let reply = vt.take_replies();
        assert!(reply.starts_with(b"\x1b[?"), "{reply:?}");
        assert!(reply.ends_with(b"c"), "{reply:?}");
        assert!(vt.take_replies().is_empty());
    }

    #[test]
    fn an_osc_52_write_reaches_the_clipboard_once() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(vt.take_clipboard().as_deref(), Some("hello"));
        assert_eq!(vt.take_clipboard(), None);
    }

    #[test]
    fn a_title_is_reported_once() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"\x1b]2;build\x07");
        assert_eq!(vt.take_title().as_deref(), Some("build"));
        assert_eq!(vt.take_title(), None);
    }

    #[test]
    fn a_synchronized_update_holds_the_frame_until_it_ends() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"old");
        assert_eq!(text(&vt.frame().rows[0]), "old");
        vt.write(b"\x1b[?2026h\x1b[2J\x1b[Hnew");
        assert!(vt.shows_change());
        assert!(vt.render_hold_deadline().is_some());
        assert_eq!(text(&vt.frame().rows[0]), "old");
        vt.write(b"\x1b[?2026l");
        assert!(vt.render_hold_deadline().is_none());
        assert_eq!(text(&vt.frame().rows[0]), "new");
    }

    fn numbered(vt: &mut Vt, n: usize) {
        for i in 0..n {
            vt.write(format!("line {i}\r\n").as_bytes());
        }
    }

    #[test]
    fn the_line_limit_bounds_the_history() {
        // The library drops whole pages, so the history sits within a page
        // (a few hundred rows) below the limit, never above it.
        let mut vt = Vt::new(120, 40, LIMITS);
        numbered(&mut vt, 12_000);
        let history = vt.history_size();
        assert!(history > 4_000 && history <= 5_000, "{history}");
    }

    #[test]
    fn the_byte_limit_bounds_the_history_below_the_line_limit() {
        let mut vt = Vt::new(
            20,
            4,
            Limits {
                lines: 5_000,
                bytes: 100_000,
            },
        );
        numbered(&mut vt, 6_000);
        let history = vt.history_size();
        assert!(history > 0 && history < 5_000, "{history}");
    }

    #[test]
    fn scrolling_reveals_older_rows_and_returns_to_the_bottom() {
        let mut vt = Vt::new(20, 4, LIMITS);
        numbered(&mut vt, 30);
        assert_eq!(vt.display_offset(), 0);
        vt.scroll_viewport(3);
        assert_eq!(vt.display_offset(), 3);
        let before = text(&vt.frame().rows[0]);
        vt.scroll_to_bottom();
        assert_eq!(vt.display_offset(), 0);
        assert_ne!(text(&vt.frame().rows[0]), before);
    }

    #[test]
    fn a_double_click_selects_the_word() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"hello world");
        vt.select(SelectKind::Word, (1, 0), (1, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("hello"));
        vt.clear_selection();
        assert_eq!(vt.selection_text(), None);
    }

    #[test]
    fn a_drag_selects_the_cells_between() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"hello world");
        vt.select(SelectKind::Cells, (6, 0), (10, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("world"));
        let frame = vt.frame();
        assert_eq!(frame.rows[0][6].bg, Some(Theme::default().selection_bg));
        assert_eq!(frame.rows[0][0].bg, None);
    }

    #[test]
    fn rows_of_text_cover_the_scrollback_and_the_screen() {
        let mut vt = Vt::new(20, 4, LIMITS);
        numbered(&mut vt, 10);
        assert_eq!(vt.screen_rows(), vt.history_size() + 4);
        let row = |vt: &Vt, y: usize| {
            vt.text_row(y)
                .unwrap()
                .cells
                .iter()
                .map(|c| c.text.clone())
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(row(&vt, 0), "line 0");
        assert_eq!(row(&vt, 9), "line 9");
        assert!(vt.text_row(vt.screen_rows()).is_none());
    }

    #[test]
    fn a_frame_says_which_rows_the_terminal_wrapped() {
        let mut vt = Vt::new(10, 4, LIMITS);
        vt.write(b"abcdefghijklmno\r\nnext");
        assert_eq!(vt.frame().wraps, vec![true, false, false, false]);
        numbered(&mut vt, 6);
        vt.scroll_viewport(2);
        assert_eq!(vt.frame().wraps.len(), 4);
    }

    #[test]
    fn a_row_the_terminal_wrapped_says_so_and_a_wide_glyph_is_one_cell() {
        let mut vt = Vt::new(10, 4, LIMITS);
        vt.write("abcdefghijklmno\r\n日本".as_bytes());
        assert!(vt.text_row(0).unwrap().wraps);
        assert!(!vt.text_row(1).unwrap().wraps);
        let row = vt.text_row(2).unwrap();
        let wide: Vec<_> = row.cells.iter().take(2).collect();
        assert_eq!(
            (wide[0].col, wide[0].text.as_str(), wide[0].wide),
            (0, "日", true)
        );
        assert_eq!(
            (wide[1].col, wide[1].text.as_str(), wide[1].wide),
            (2, "本", true)
        );
        assert_eq!(row.cells[2].text, " ");
    }

    #[test]
    fn a_selection_is_by_screen_row_and_survives_scrolling() {
        let mut vt = Vt::new(20, 4, LIMITS);
        numbered(&mut vt, 10);
        assert!(vt.viewport_top() > 0);
        vt.select(SelectKind::Cells, (0, 2), (5, 2));
        assert_eq!(vt.selection_text().as_deref(), Some("line 2"));
        vt.write(b"more\r\nmore\r\n");
        assert_eq!(vt.selection_text().as_deref(), Some("line 2"));
        // Backwards works too.
        vt.select(SelectKind::Cells, (5, 3), (0, 3));
        assert_eq!(vt.selection_text().as_deref(), Some("line 3"));
    }

    #[test]
    fn dragging_after_a_double_click_grows_by_words_and_a_triple_click_takes_the_line() {
        let mut vt = Vt::new(30, 4, LIMITS);
        vt.write(b"alpha beta gamma\r\nsecond");
        vt.select(SelectKind::Word, (1, 0), (1, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("alpha"));
        vt.select(SelectKind::Word, (1, 0), (8, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("alpha beta"));
        vt.select(SelectKind::Word, (12, 0), (1, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("alpha beta gamma"));
        vt.select(SelectKind::Line, (3, 0), (3, 0));
        assert_eq!(vt.selection_text().as_deref(), Some("alpha beta gamma"));
        vt.select(SelectKind::Line, (3, 0), (2, 1));
        assert_eq!(
            vt.selection_text().as_deref(),
            Some("alpha beta gamma\nsecond")
        );
    }

    #[test]
    fn the_modes_the_program_set_are_readable() {
        let mut vt = Vt::new(20, 4, LIMITS);
        assert!(!vt.mouse_tracking() && !vt.alt_screen() && !vt.mode(2004));
        vt.write(b"\x1b[?1000h\x1b[?1049h\x1b[?2004h");
        assert!(vt.mouse_tracking() && vt.alt_screen() && vt.mode(2004));
    }

    #[test]
    fn an_unchanged_row_is_the_same_allocation_in_the_next_frame() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"one\r\ntwo");
        let first = vt.frame();
        vt.write(b"!");
        let second = vt.frame();
        assert!(Arc::ptr_eq(&first.rows[0], &second.rows[0]));
        assert!(!Arc::ptr_eq(&first.rows[1], &second.rows[1]));
    }

    fn rebuilt(vt: &mut Vt) -> Vt {
        let mut again = Vt::new(vt.cols, vt.rows, LIMITS);
        again.write(&vt.dump_vt());
        again
    }

    fn same_screen(a: &mut Vt, b: &mut Vt) {
        let (fa, fb) = (a.frame(), b.frame());
        assert_eq!(fa.rows.len(), fb.rows.len());
        for (i, (ra, rb)) in fa.rows.iter().zip(&fb.rows).enumerate() {
            assert_eq!(ra, rb, "row {i}");
        }
        assert_eq!(fa.cursor_cell, fb.cursor_cell);
        assert_eq!(a.history_size(), b.history_size());
    }

    #[test]
    fn a_dump_rebuilds_history_screen_cursor_and_modes() {
        let mut vt = Vt::new(12, 4, LIMITS);
        for i in 0..20 {
            vt.write(format!("\x1b[3{}mline {i}\x1b[0m\r\n", i % 7).as_bytes());
        }
        // A wrapped line, styled text, then the cursor left mid-row with modes set.
        vt.write(
            b"abcdefghijklmnopqrstuvwx\r\n\x1b[1;4mbold\x1b[0m ok\x1b[?2004h\x1b[?1000h\x1b[2;3H",
        );
        let mut again = rebuilt(&mut vt);
        same_screen(&mut vt, &mut again);
        assert!(again.history_size() >= 17);
        assert!(again.mode(2004) && again.mouse_tracking());
        // The wrap survived: widening joins the two rows again.
        vt.resize(30, 4);
        again.resize(30, 4);
        same_screen(&mut vt, &mut again);
    }

    #[test]
    fn a_dump_rebuilds_the_alternate_screen() {
        let mut vt = Vt::new(12, 4, LIMITS);
        vt.write(b"primary\r\n\x1b[?1049h\x1b[Halt");
        let mut again = rebuilt(&mut vt);
        assert!(again.alt_screen());
        assert_eq!(text(&again.frame().rows[0]), "alt");
        // The primary screen under the running program is not in the dump: the
        // program's next redraw of it is what restores it.
        again.write(b"\x1b[?1049l");
        assert_eq!(text(&again.frame().rows[0]), "");
    }

    #[test]
    fn a_dump_keeps_the_blank_row_the_cursor_sits_on() {
        let mut vt = Vt::new(12, 4, LIMITS);
        numbered(&mut vt, 20);
        let mut again = rebuilt(&mut vt);
        same_screen(&mut vt, &mut again);
        assert_eq!(text(&again.frame().rows[3]), "");
        assert_eq!(text(&again.frame().rows[2]), "line 19");
    }

    #[test]
    fn a_dump_of_a_cleared_screen_keeps_the_history() {
        let mut vt = Vt::new(12, 4, LIMITS);
        numbered(&mut vt, 20);
        vt.write(b"\x1b[H\x1b[2J");
        let mut again = rebuilt(&mut vt);
        same_screen(&mut vt, &mut again);
        assert!(again.history_size() >= 17);
        assert!(again.frame().rows.iter().all(|r| text(r).is_empty()));
    }

    #[test]
    fn a_row_with_only_a_background_keeps_its_place_in_a_dump() {
        // The dump drops the background of an empty row; the rows around it
        // must still sit where they were.
        let mut vt = Vt::new(12, 4, LIMITS);
        numbered(&mut vt, 10);
        vt.write(b"\x1b[44m\x1b[K\x1b[0m");
        let mut again = rebuilt(&mut vt);
        let (a, b) = (vt.frame(), again.frame());
        for i in 0..4 {
            assert_eq!(text(&a.rows[i]), text(&b.rows[i]), "row {i}");
        }
        assert_eq!(a.cursor_cell, b.cursor_cell);
        assert_eq!(vt.history_size(), again.history_size());
    }

    #[test]
    fn a_dump_of_a_short_screen_does_not_scroll() {
        let mut vt = Vt::new(12, 4, LIMITS);
        vt.write(b"one\r\ntwo");
        let mut again = rebuilt(&mut vt);
        same_screen(&mut vt, &mut again);
        assert_eq!(text(&again.frame().rows[0]), "one");
        assert_eq!(again.history_size(), 0);
    }

    #[test]
    fn the_cursor_cell_follows_output_and_the_scroll() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"abc");
        assert_eq!(vt.cursor_cell(), Some((3, 0)));
        numbered(&mut vt, 30);
        let live = vt.cursor_cell();
        assert!(live.is_some());
        vt.scroll_viewport(20);
        assert_eq!(vt.cursor_cell(), None, "scrolled off the bottom edge");
        vt.scroll_to_bottom();
        assert_eq!(vt.cursor_cell(), live);
    }

    #[test]
    fn the_cells_a_wide_glyph_leaves_are_spacers() {
        let mut vt = Vt::new(5, 3, LIMITS);
        // 日 covers cols 2-3: its tail cell is a spacer. Addressed to the
        // last column it cannot fit there: that column is a spacer head
        // and the glyph lands on the next row instead.
        vt.write("ab日\x1b[1;5H日".as_bytes());
        let frame = vt.frame();
        assert!(frame.rows[0][3].spacer, "the tail cell");
        assert!(frame.rows[0][4].spacer, "the head the next glyph left");
        assert!(!frame.rows[0][2].spacer && frame.rows[0][2].wide);
        assert_eq!(text(&frame.rows[1]), "日");
    }

    #[test]
    fn a_vt_can_move_to_another_thread() {
        let mut vt = Vt::new(20, 4, LIMITS);
        vt.write(b"before ");
        let vt = std::thread::spawn(move || {
            vt.write(b"after");
            vt
        })
        .join()
        .unwrap();
        let mut vt = vt;
        assert_eq!(text(&vt.frame().rows[0]), "before after");
    }
}

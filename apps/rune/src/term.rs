//! The live terminal: the daemon's PTY bytes -> alacritty_terminal grid ->
//! styled rich-text runs -> a scrolling line column.
//!
//! The grid is the real thing: `Term` parses the ANSI stream the daemon
//! replays and streams, and the visible rows become `TermLine`s. The
//! palette is the Tango ansi16 set the real TerminalController renders on
//! #1e1e1e, ported from the verified rune-spike `fidelity.rs`.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, Processor};
use app_core::error::{BridgeError, Code};
use protocol::message::Response;
use protocol::session::SessionId;
use rune::{Color, El, LineCap, Link, LinkPaint, Shape, Span, TextStyle, text};

use crate::daemon::{self, Output, SubscribeReply};
use crate::theme::Ink;

// ---------------------------------------------------------------------------
// Terminal: ANSI bytes -> alacritty_terminal grid -> styled runs.
// ---------------------------------------------------------------------------

/// One styled run of terminal text.
#[derive(Clone, Debug, PartialEq)]
struct TermRun {
    text: String,
    color: Color,
    weight: f32,
    italic: bool,
    underline: Option<Color>,
}

/// A span of cells sharing one background colour: the first cell, one past
/// the last, and the colour to paint under the text. Adjacent cells with
/// the same resolved colour merge into one span, so a solid block costs a
/// single shape fill, not a fill per cell.
#[derive(Clone, Debug, PartialEq)]
struct Back {
    start: usize,
    end: usize,
    color: Color,
}

/// The terminal cursor parked on this row: the cell it sits on, how many
/// cells it covers (a wide glyph takes two) and how it is drawn.
#[derive(Clone, Debug, PartialEq)]
struct TermCursor {
    col: usize,
    span: usize,
    shape: CursorShape,
    color: Color,
}

/// One terminal row: styled runs of text, cell backgrounds and the cursor,
/// left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct TermLine {
    runs: Vec<TermRun>,
    backs: Vec<Back>,
    cursor: Option<TermCursor>,
    /// The text runs on into the next row: the terminal wrapped it there.
    wrapped: bool,
}

impl TermLine {
    /// The row's text, runs joined.
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }
}

/// The grid the terminal renders at. The daemon's PTY is resized to match
/// it, so the columns line up with what the user sees.
pub const TERM_COLS: usize = 120;
pub const TERM_ROWS: usize = 42;

/// The grid size, as alacritty wants it.
#[derive(Clone, Copy)]
struct Size {
    screen_lines: usize,
    columns: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// How long the output has to be quiet before the grid is repainted: a
/// full-screen program redraws in bursts, and repainting every chunk
/// would show the screen rebuilding itself line by line.
const QUIET: Duration = Duration::from_millis(16);
/// The longest a repaint may be held back, so a program that never stops
/// writing still shows its output.
const MAX_HOLD: Duration = Duration::from_millis(200);

/// The live terminal of the attached session: the grid, the ANSI parser and
/// the daemon's output stream.
pub struct Terminal {
    /// The session this terminal is attached to.
    pub session: SessionId,
    /// The daemon's output stream for this session.
    chunks: Receiver<Output>,
    /// The daemon's answer to `terminal.subscribe`. It is taken once: the
    /// reply channel closes behind the answer, and a closed channel is no
    /// reason to drop the attachment.
    ready: Option<Receiver<Result<Response, BridgeError>>>,
    /// The stream offset the parser consumed through — the flow-control
    /// watermark the daemon is acked against. A resubscribe replays the
    /// retained buffer, so chunks at or before this offset are dropped and
    /// later ones are trimmed to their unparsed tail. It only advances over
    /// bytes the parser actually consumed, so an acknowledgement never
    /// claims output that was not processed.
    pub next_seq: u64,
    /// The last byte acknowledged to the daemon.
    acked_seq: u64,
    term: Term<VoidListener>,
    parser: Processor,
    /// The rendered rows of the visible grid.
    lines: Vec<TermLine>,
    /// New output arrived since the last render.
    dirty: bool,
    /// The grid was drawn at least once: only a burst of NEW output waits
    /// for the output to settle, the first draw does not.
    rendered: bool,
    /// When the last chunk arrived.
    last: Instant,
    /// When the current burst of unrendered output started. `None` while
    /// nothing new waits to be drawn, so the first draw and a resize are
    /// never held back.
    burst: Option<Instant>,
    /// Wheel points not yet worth a whole line: a trackpad sends many
    /// small deltas.
    scroll_rem: f32,
}

impl Terminal {
    /// Attach to `session`: the grid starts empty and the daemon's stream
    /// feeds it. `ready` carries the daemon's answer to the subscribe.
    pub fn new(
        session: SessionId,
        chunks: Receiver<Output>,
        ready: Receiver<Result<Response, BridgeError>>,
    ) -> Terminal {
        let size = Size {
            screen_lines: TERM_ROWS,
            columns: TERM_COLS,
        };
        Terminal {
            session,
            chunks,
            ready: Some(ready),
            next_seq: 0,
            acked_seq: 0,
            term: Term::new(Config::default(), &size, VoidListener),
            parser: Processor::new(),
            lines: Vec::new(),
            dirty: true,
            rendered: false,
            last: Instant::now(),
            burst: None,
            scroll_rem: 0.0,
        }
    }

    /// Scroll the history by wheel points (positive reveals older lines),
    /// `line_h` points to a line. Drawn at once; new output leaves the
    /// view where it is while it is scrolled up.
    pub fn scroll(&mut self, dy: f32, line_h: f32) {
        self.scroll_rem += dy / line_h.max(1.0);
        let lines = self.scroll_rem.trunc();
        self.scroll_rem -= lines;
        if lines != 0.0 {
            self.term.scroll_display(Scroll::Delta(lines as i32));
            self.dirty = true;
            self.burst = None;
        }
    }

    /// Whether the program asked for pastes wrapped in ESC[200~ … ESC[201~.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Back to the live screen — where typing lands.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_rem = 0.0;
        if self.term.grid().display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
            self.dirty = true;
            self.burst = None;
        }
    }

    /// The daemon's answer to `terminal.subscribe`: `Err` means the attach
    /// failed, `None` means the answer is still on its way.
    pub fn subscribe_reply(&mut self) -> Option<Result<SubscribeReply, BridgeError>> {
        let ready = self.ready.take()?;
        let reply = match ready.try_recv() {
            Ok(Ok(response)) => Some(match daemon::result_of(&response) {
                Ok(value) => serde_json::from_value::<SubscribeReply>(value.clone()).map_err(|error| {
                    BridgeError::new(
                        Code::Daemon(protocol::message::ErrorCode::Malformed),
                        format!("the daemon's subscribe reply did not fit the expected shape: {error}"),
                    )
                }),
                Err(error) => Err(error),
            }),
            Ok(Err(error)) => Some(Err(error)),
            Err(TryRecvError::Empty) => {
                // The answer is still on its way: keep the channel.
                self.ready = Some(ready);
                None
            }
            // The channel closed: the answer was already read (the reply
            // channel closes behind its answer) or the daemon is gone.
            Err(TryRecvError::Disconnected) => None,
        };
        // The reply's `next_seq` is where the live stream starts, not a
        // watermark: the replayed bytes still have to arrive and be
        // parsed before they can be acknowledged. What can be checked is
        // the pair's consistency — the replay cannot start past the live
        // stream's next byte.
        if let Some(Ok(subscribe)) = &reply
            && subscribe.replay_from > subscribe.next_seq
        {
            return Some(Err(BridgeError::new(
                Code::Daemon(protocol::message::ErrorCode::Malformed),
                format!(
                    "the daemon's subscribe reply is inconsistent: replay_from {} is past next_seq {}",
                    subscribe.replay_from, subscribe.next_seq
                ),
            )));
        }
        reply
    }
    /// Feed the bytes the daemon sent. Returns whether the grid changed.
    pub fn feed(&mut self) -> bool {
        let mut changed = false;
        while let Ok(chunk) = self.chunks.try_recv() {
            let end = chunk.seq + chunk.data.len() as u64;
            if end <= self.next_seq {
                // A resubscribe replays bytes the parser already has.
                continue;
            }
            // The replay can overlap the parsed tail: feed only the part
            // that was not seen yet. A chunk ahead of `next_seq` is a
            // gap the daemon already dropped — parse it whole.
            let skip = self.next_seq.saturating_sub(chunk.seq) as usize;
            let data = &chunk.data[skip.min(chunk.data.len())..];
            if data.is_empty() {
                continue;
            }
            self.parser.advance(&mut self.term, data);
            self.next_seq = end;
            self.last = Instant::now();
            changed = true;
        }
        if changed {
            self.dirty = true;
            if self.burst.is_none() {
                self.burst = Some(Instant::now());
            }
        }
        changed
    }

    /// How long the repaint may be held back: the output has to settle
    /// first, so a full-screen program is drawn once instead of line by
    /// line, but a stream that never goes quiet is still drawn once the
    /// burst is `MAX_HOLD` old. `None` means "repaint now".
    pub fn hold(&self) -> Option<Duration> {
        if !self.dirty || !self.rendered {
            return None;
        }
        let quiet = QUIET.saturating_sub(self.last.elapsed());
        if quiet.is_zero() {
            // The output settled.
            return None;
        }
        let Some(burst) = self.burst else {
            // Dirty without a burst is a resize: draw now.
            return None;
        };
        let hold = MAX_HOLD.saturating_sub(burst.elapsed());
        if hold.is_zero() {
            // The burst is as old as a repaint may be.
            return None;
        }
        Some(quiet.min(hold))
    }

    /// The rendered rows of the visible grid, repainted when the output
    /// settled.
    pub fn lines(&mut self, ink: &Ink) -> &[TermLine] {
        if self.dirty && self.hold().is_none() {
            self.lines = render_lines(&self.term, ink);
            self.dirty = false;
            self.rendered = true;
            self.burst = None;
        }
        &self.lines
    }

    /// Take a fresh subscription after the connection was replaced: the
    /// grid stays, the output stream is new.
    pub fn resubscribe(
        &mut self,
        chunks: Receiver<Output>,
        ready: Receiver<Result<Response, BridgeError>>,
    ) {
        self.chunks = chunks;
        self.ready = Some(ready);
    }

    /// Resize the grid: the PTY follows the pane, so what the session
    /// draws is exactly what the pane shows.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }
        let size = Size {
            screen_lines: rows,
            columns: cols,
        };
        self.term.resize(size);
        self.dirty = true;
        // A resized grid is drawn at once: no burst is settling.
        self.burst = None;
    }

    /// The grid size the terminal renders at.
    pub fn size(&self) -> (usize, usize) {
        (self.term.grid().columns(), self.term.grid().screen_lines())
    }

    /// The cursor's cell on the visible screen, (row, column) — where the
    /// input method's candidate window and the preedit go.
    pub fn cursor_cell(&self) -> Option<(usize, usize)> {
        let grid = self.term.grid();
        let row = grid.cursor.point.line.0 + grid.display_offset() as i32;
        let col = grid.cursor.point.column.0;
        ((0..grid.screen_lines() as i32).contains(&row) && col < grid.columns())
            .then_some((row as usize, col))
    }

    /// The palette changed while the grid did not: the cached rows repaint
    /// in the new colours on the next `lines` call. The parser, the stream
    /// offsets and the PTY size are untouched — no resize is faked.
    pub fn refresh_theme(&mut self) {
        self.dirty = true;
        // A colour swap is not output still settling: draw now instead of
        // outwaiting the burst hold.
        self.burst = None;
    }

    /// The bytes the daemon sent that were not acknowledged yet. A
    /// flow-controlled stream waits for this.
    pub fn unacked(&self) -> u64 {
        self.next_seq.saturating_sub(self.acked_seq)
    }

    /// Remember the bytes that were acknowledged.
    pub fn acknowledge(&mut self, upto_seq: u64) {
        self.acked_seq = upto_seq;
    }
}

/// The 16 ANSI colours exactly as xterm.js ships them. Goko's TerminalController
/// only overrides background/foreground/cursor, so the real terminal renders
/// this Tango palette on #1e1e1e — using it here is the fidelity-correct choice.
fn ansi16(i: u8) -> Color {
    const T: [u32; 16] = [
        0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753,
        0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
    ];
    let v = T[(i & 15) as usize];
    Color {
        r: rune::color::srgb_decode(((v >> 16) & 0xff) as f32 / 255.0),
        g: rune::color::srgb_decode(((v >> 8) & 0xff) as f32 / 255.0),
        b: rune::color::srgb_decode((v & 0xff) as f32 / 255.0),
        a: 1.0,
    }
}

/// A configured terminal colour (the `colors` table) as a rune colour.
fn rgb_color(rgb: alacritty_terminal::vte::ansi::Rgb) -> Color {
    Color {
        r: rune::color::srgb_decode(rgb.r as f32 / 255.0),
        g: rune::color::srgb_decode(rgb.g as f32 / 255.0),
        b: rune::color::srgb_decode(rgb.b as f32 / 255.0),
        a: 1.0,
    }
}

fn ansi_color(c: alacritty_terminal::vte::ansi::Color, ink: &Ink) -> Color {
    use alacritty_terminal::vte::ansi::{Color as A, NamedColor as N};
    match c {
        A::Named(n) => match n {
            N::Black => ansi16(0),
            N::Red => ansi16(1),
            N::Green => ansi16(2),
            N::Yellow => ansi16(3),
            N::Blue => ansi16(4),
            N::Magenta => ansi16(5),
            N::Cyan => ansi16(6),
            N::White => ansi16(7),
            N::BrightBlack => ansi16(8),
            N::BrightRed => ansi16(9),
            N::BrightGreen => ansi16(10),
            N::BrightYellow => ansi16(11),
            N::BrightBlue => ansi16(12),
            N::BrightMagenta => ansi16(13),
            N::BrightCyan => ansi16(14),
            N::BrightWhite => ansi16(15),
            N::Foreground => ink.text,
            N::Background => ink.bg,
            _ => ink.dim,
        },
        A::Spec(rgb) => rgb_color(rgb),
        A::Indexed(i) => {
            let i = i as usize;
            match i {
                0..=15 => ansi16(i as u8),
                16..=231 => {
                    let n = i - 16;
                    let f = |v: usize| match v {
                        0 => 0,
                        1 => 95,
                        2 => 135,
                        3 => 175,
                        4 => 215,
                        _ => 255,
                    };
                    let (r, g, b) = (f(n / 36), f((n / 6) % 6), f(n % 6));
                    Color {
                        r: rune::color::srgb_decode(r as f32 / 255.0),
                        g: rune::color::srgb_decode(g as f32 / 255.0),
                        b: rune::color::srgb_decode(b as f32 / 255.0),
                        a: 1.0,
                    }
                }
                _ => {
                    let v = (8 + (i.saturating_sub(232)) * 10).min(238);
                    let l = rune::color::srgb_decode(v as f32 / 255.0);
                    Color {
                        r: l,
                        g: l,
                        b: l,
                        a: 1.0,
                    }
                }
            }
        }
    }
}

/// Turn the terminal's visible grid rows into `TermLine`s.
fn render_lines(term: &Term<VoidListener>, ink: &Ink) -> Vec<TermLine> {
    use alacritty_terminal::vte::ansi::{Color as A, NamedColor as N};
    let content = term.renderable_content();
    let rows = term.grid().screen_lines();
    let cols = term.grid().columns();
    // `display_iter` and the cursor speak in grid lines, where the visible
    // screen is `-display_offset..screen_lines-display_offset`; the row a
    // cell lands on is its line plus the scroll offset.
    let display_offset = content.display_offset as i32;
    let mut grid: Vec<Vec<&Cell>> = (0..rows).map(|_| Vec::with_capacity(cols)).collect();
    for indexed in content.display_iter {
        let row = indexed.point.line.0 + display_offset;
        if (0..rows as i32).contains(&row) {
            grid[row as usize].push(indexed.cell);
        }
    }

    // The cursor alacritty would draw: `renderable_content` already folds
    // in the show/hide mode, vi mode and the wide-char spacer adjustment,
    // and `Hidden` means there is nothing to paint — no fake trailing
    // block. A block cursor is drawn by swapping the cell's colours in the
    // row walk below; the other shapes become a `TermCursor` on their row.
    let cursor = match content.cursor.shape {
        CursorShape::Hidden => None,
        shape => {
            let point = content.cursor.point;
            let (line, col) = (point.line.0 + display_offset, point.column.0);
            if !(0..rows as i32).contains(&line) || col >= cols {
                None
            } else {
                // A wide glyph's cursor covers both of its cells.
                let span = usize::from(term.grid()[point].flags.contains(Flags::WIDE_CHAR)) + 1;
                Some((line as usize, col, span, shape))
            }
        }
    };
    // A colour the terminal was configured with wins over the cell's ink.
    let cursor_color = content.colors[N::Cursor].map(rgb_color);

    grid.into_iter()
        .enumerate()
        .map(|(row, cells)| {
            let mut runs: Vec<TermRun> = Vec::new();
            let mut backs: Vec<Back> = Vec::new();
            let mut cur = String::new();
            let mut cur_style: Option<(Color, f32, bool, Option<Color>)> = None;
            // The open background span: its first cell and colour.
            let mut back_start: Option<usize> = None;
            let mut back_color = Color::default();
            let cursor_here = cursor.filter(|(line, ..)| *line == row);
            // The ink under the cursor cell, for the non-block shapes.
            let mut cursor_fg: Option<Color> = None;
            for (col, cell) in cells.iter().enumerate() {
                // INVERSE video swaps the cell's colours, as a terminal does.
                let (fg_ansi, bg_ansi) = if cell.flags.contains(Flags::INVERSE) {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                let mut fg = ansi_color(fg_ansi, ink);
                if cell.flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
                    // xterm.js draws DIM as the colour at 50% opacity over the
                    // background (.xterm-dim { color: multiplyOpacity(.5) }).
                    fg = fg.mix(ink.bg, 0.5);
                }
                let mut bg = match bg_ansi {
                    A::Named(N::Background) => None,
                    ansi => Some(ansi_color(ansi, ink)),
                };
                if cursor_here.is_some_and(|(_, at, ..)| at == col) {
                    cursor_fg = Some(fg);
                }
                // A block cursor is the cell drawn inverse: its ink becomes
                // the background and its background the ink.
                if cursor_here.is_some_and(|(_, at, span, shape)| {
                    shape == CursorShape::Block && (at..at + span).contains(&col)
                }) {
                    let inked = bg.unwrap_or(ink.bg);
                    bg = Some(fg);
                    fg = inked;
                }
                // Merge the cell into the open background span, or close it.
                match bg {
                    Some(color) if back_start.is_some() && color == back_color => {}
                    Some(color) => {
                        if let Some(start) = back_start.replace(col) {
                            backs.push(Back {
                                start,
                                end: col,
                                color: back_color,
                            });
                        }
                        back_color = color;
                    }
                    None => {
                        if let Some(start) = back_start.take() {
                            backs.push(Back {
                                start,
                                end: col,
                                color: back_color,
                            });
                        }
                    }
                }
                // Wide-char spacers carry no glyph of their own — the wide
                // glyph already spans their cells, so columns don't drift.
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    cur.push(' ');
                    continue;
                }
                let weight = if cell.flags.intersects(Flags::BOLD | Flags::BOLD_ITALIC) {
                    700.0
                } else {
                    400.0
                };
                let italic = cell.flags.contains(Flags::ITALIC);
                let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| {
                    cell.underline_color()
                        .map(|ansi| ansi_color(ansi, ink))
                        .unwrap_or(fg)
                });
                let style = (fg, weight, italic, underline);
                if cur_style != Some(style) {
                    if !cur.is_empty() {
                        let (color, weight, italic, underline) = cur_style.unwrap();
                        runs.push(TermRun {
                            text: std::mem::take(&mut cur),
                            color,
                            weight,
                            italic,
                            underline,
                        });
                    }
                    cur_style = Some(style);
                }
                cur.push(cell.c);
                // Combining marks ride on their base cell, zero-width.
                for c in cell.zerowidth().into_iter().flatten() {
                    cur.push(*c);
                }
            }
            if let Some(start) = back_start.take() {
                backs.push(Back {
                    start,
                    end: cells.len(),
                    color: back_color,
                });
            }
            if !cur.is_empty() {
                let (color, weight, italic, underline) = cur_style.unwrap();
                runs.push(TermRun {
                    text: cur,
                    color,
                    weight,
                    italic,
                    underline,
                });
            }
            if runs.is_empty() {
                runs.push(TermRun {
                    text: " ".into(),
                    color: ink.text,
                    weight: 400.0,
                    italic: false,
                    underline: None,
                });
            }
            let cursor = cursor_here.and_then(|(_, col, span, shape)| match shape {
                CursorShape::Hidden | CursorShape::Block => None,
                _ => Some(TermCursor {
                    col,
                    span,
                    shape,
                    color: cursor_color.or(cursor_fg).unwrap_or(ink.text),
                }),
            });
            TermLine {
                runs,
                backs,
                cursor,
                wrapped: cells
                    .last()
                    .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE)),
            }
        })
        .collect()
}

/// The bytes a key press sends to the PTY: the ANSI sequences a terminal
/// expects. `None` for the keys a terminal has no use for (function keys,
/// the modifier keys themselves).
pub fn key_bytes(ev: &rune::KeyEvent) -> Option<Vec<u8>> {
    use rune::Key;
    // ⌘ is the app's, except the line-editing chords Terminal.app and iTerm
    // map to readline: ⌘←/⌘→ line start/end (^A/^E), ⌘⌫ kill line (^U).
    if ev.mods.cmd && !ev.mods.ctrl {
        return match ev.key {
            Key::Left => Some(vec![0x01]),
            Key::Right => Some(vec![0x05]),
            Key::Backspace => Some(vec![0x15]),
            _ => None,
        };
    }
    // xterm's modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4).
    let m = (ev.mods.shift as u8) | (ev.mods.alt as u8) << 1 | (ev.mods.ctrl as u8) << 2;
    let m = (m != 0).then_some(m + 1);
    // CSI A–D/H/F, `CSI 1;m X` when modified; CSI n~, `CSI n;m ~` when modified.
    let cursor = |c: char| match m {
        Some(m) => format!("\x1b[1;{m}{c}"),
        None => format!("\x1b[{c}"),
    };
    let tilde = |n: u8| match m {
        Some(m) => format!("\x1b[{n};{m}~"),
        None => format!("\x1b[{n}~"),
    };
    // ⌥←/⌥→ move by word: ESC b / ESC f, what readline and Claude Code read.
    let word = ev.mods.alt && !ev.mods.shift && !ev.mods.ctrl;
    match &ev.key {
        Key::Text(text) => {
            // Ctrl+letter is a control byte; everything else is its UTF-8.
            if ev.mods.ctrl
                && !ev.mods.cmd
                && let Some(ch) = text.chars().next()
                && text.chars().count() == 1
                && (ch.is_ascii_alphabetic() || " @[\\]^_".contains(ch))
            {
                return Some(vec![(ch.to_ascii_lowercase() as u8) & 0x1f]);
            }
            // Ctrl+- and Ctrl+/ are xterm's ^_ (readline undo).
            if ev.mods.ctrl && !ev.mods.cmd && matches!(text.as_str(), "-" | "/") {
                return Some(vec![0x1f]);
            }
            // ⌥+letter is Meta: ESC then the letter (Claude Code's ⌥P model
            // switch, readline's ⌥B/⌥F/⌥D). `terminal_key` swaps in the letter
            // the key sits on, since macOS types `π` for ⌥P.
            if ev.mods.alt
                && !ev.mods.ctrl
                && text.len() == 1
                && text.starts_with(|c: char| c.is_ascii_alphabetic())
            {
                return Some([&[0x1b][..], text.as_bytes()].concat());
            }
            Some(text.as_bytes().to_vec())
        }
        // Shift/Option+Enter is ESC CR, the sequence Claude Code (and what
        // `/terminal-setup` binds Shift+Enter to) reads as a newline; a
        // bare terminal cannot tell it from Enter's CR otherwise.
        Key::Enter if ev.mods.shift || ev.mods.alt => Some(vec![0x1b, b'\r']),
        Key::Enter => Some(vec![b'\r']),
        // ⌥⌫ deletes a word: ESC DEL.
        Key::Backspace if ev.mods.alt => Some(vec![0x1b, 0x7f]),
        Key::Backspace => Some(vec![0x7f]),
        // Shift+Tab is back-tab (CSI Z): how Claude Code-style TUIs cycle modes.
        Key::Tab if ev.mods.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(vec![b'\t']),
        Key::Escape => Some(vec![0x1b]),
        Key::Left if word => Some(vec![0x1b, b'b']),
        Key::Right if word => Some(vec![0x1b, b'f']),
        Key::Left => Some(cursor('D').into_bytes()),
        Key::Right => Some(cursor('C').into_bytes()),
        Key::Up => Some(cursor('A').into_bytes()),
        Key::Down => Some(cursor('B').into_bytes()),
        Key::Home => Some(cursor('H').into_bytes()),
        Key::End => Some(cursor('F').into_bytes()),
        Key::Delete => Some(tilde(3).into_bytes()),
        Key::PageUp => Some(tilde(5).into_bytes()),
        Key::PageDown => Some(tilde(6).into_bytes()),
        Key::Insert => Some(tilde(2).into_bytes()),
        _ => None,
    }
}

/// 13 pt monospace on a 16 pt line — xterm fontSize 13 with a ~16 px cell.
const TERM_FONT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(16.0);

/// What a terminal row's text links to.
#[derive(Debug, PartialEq)]
pub enum LinkKind {
    Url,
    File { path: String, line: Option<u64> },
}

/// The `http(s)` URLs and path-like tokens (`dir/file.ext[:line[:col]]`) in a
/// row, as byte ranges of `text`. A URL drops trailing sentence punctuation and
/// an unbalanced closing bracket. A file token needs an extension and either a
/// `/` or a line suffix.
// ponytail: a bare `README.md` is not linked (too many false hits such as
// `node.js`). A URL the terminal wrapped is joined by `term_lines_el`; one a
// program broke with its own newline is still cut at the row end.
pub fn find_links(text: &str) -> Vec<(std::ops::Range<usize>, LinkKind)> {
    let mut out: Vec<(std::ops::Range<usize>, LinkKind)> = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("http") {
        let start = from + i;
        let rest = &text[start..];
        let scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let len = rest
            .find(|c: char| c.is_whitespace() || "<>\"'`".contains(c))
            .unwrap_or(rest.len());
        let mut end = start + len;
        while end > start + scheme {
            let url = &text[start..end];
            let last = url.chars().last().unwrap_or(' ');
            let unbalanced =
                |open: char, close: char| url.matches(close).count() > url.matches(open).count();
            let drop = ".,;:!?".contains(last)
                || (last == ')' && unbalanced('(', ')'))
                || (last == ']' && unbalanced('[', ']'))
                || (last == '}' && unbalanced('{', '}'));
            if !drop {
                break;
            }
            end -= last.len_utf8();
        }
        if end > start + scheme {
            out.push((start..end, LinkKind::Url));
        }
        from = end.max(start + scheme);
    }
    let urls = out.len();

    let is_break = |c: char| c.is_whitespace() || "()[]{}<>\"'`,;".contains(c);
    let mut token: Option<usize> = None;
    let mut ends: Vec<(usize, usize)> = Vec::new();
    for (i, c) in text.char_indices() {
        match (token, is_break(c)) {
            (None, false) => token = Some(i),
            (Some(start), true) => {
                ends.push((start, i));
                token = None;
            }
            _ => {}
        }
    }
    if let Some(start) = token {
        ends.push((start, text.len()));
    }
    for (start, end) in ends {
        let raw = text[start..end].trim_end_matches(['.', ':']);
        let end = start + raw.len();
        if out[..urls]
            .iter()
            .any(|(r, _)| r.start < end && start < r.end)
        {
            continue;
        }
        let (mut path, mut nums) = (raw, Vec::new());
        for _ in 0..2 {
            match path.rsplit_once(':') {
                Some((head, tail))
                    if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    nums.extend(tail.parse::<u64>().ok());
                    path = head;
                }
                _ => break,
            }
        }
        let line = nums.last().copied();
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext_ok = name.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=10).contains(&ext.len())
                && ext.bytes().all(|b| b.is_ascii_alphanumeric())
                && ext.bytes().any(|b| b.is_ascii_alphabetic())
        });
        if ext_ok
            && !path.contains("://")
            && !path.starts_with('-')
            && (path.contains('/') || line.is_some())
        {
            out.push((
                start..end,
                LinkKind::File {
                    path: path.to_string(),
                    line,
                },
            ));
        }
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// The id a link carries to `State::open_terminal_link`: `u\turl` or
/// `f\tsession\tpath\tline`.
fn link_id(kind: &LinkKind, text: &str, session: &str) -> String {
    match kind {
        LinkKind::Url => format!("u\t{text}"),
        LinkKind::File { path, line } => format!(
            "f\t{session}\t{path}\t{}",
            line.map(|n| n.to_string()).unwrap_or_default()
        ),
    }
}

fn term_line_el(
    line: &TermLine,
    ink: &Ink,
    cell_w: f32,
    line_h: f32,
    links: Vec<(std::ops::Range<usize>, String)>,
) -> El {
    let mut content = String::new();
    let mut spans = Vec::with_capacity(line.runs.len());
    let mut at = 0usize;
    for run in &line.runs {
        let end = at + run.text.len();
        let mut sp = Span::new(at, end).color(run.color);
        if run.weight != 400.0 {
            sp = sp.weight(run.weight);
        }
        if run.italic {
            sp = sp.italic();
        }
        if let Some(underline) = run.underline {
            sp = sp.underline(underline);
        }
        spans.push(sp);
        content.push_str(&run.text);
        at = end;
    }
    // The cell backgrounds and the cursor are shape fills in the text
    // element's own points: they paint under the glyphs and line up with
    // the cells by construction. One shape per row keeps a solid block to
    // a single fill however many cells it covers.
    let mut shape = Shape::new();
    for back in &line.backs {
        let x0 = back.start as f32 * cell_w;
        let x1 = back.end as f32 * cell_w;
        shape = shape.fill(
            vec![[x0, 0.0], [x1, 0.0], [x1, line_h], [x0, line_h]],
            back.color,
        );
    }
    if let Some(cursor) = &line.cursor {
        let x = cursor.col as f32 * cell_w;
        let w = cursor.span as f32 * cell_w;
        shape = match cursor.shape {
            CursorShape::Block | CursorShape::Hidden => shape,
            CursorShape::Underline => {
                let t = (line_h * 0.15).max(1.0);
                shape.fill(
                    vec![
                        [x, line_h - t],
                        [x + w, line_h - t],
                        [x + w, line_h],
                        [x, line_h],
                    ],
                    cursor.color,
                )
            }
            CursorShape::Beam => {
                let t = (cell_w * 0.22).max(1.0);
                shape.fill(
                    vec![[x, 0.0], [x + t, 0.0], [x + t, line_h], [x, line_h]],
                    cursor.color,
                )
            }
            CursorShape::HollowBlock => {
                let i = 1.0;
                shape.stroke_with(
                    vec![
                        [x + i, i],
                        [x + w - i, i],
                        [x + w - i, line_h - i],
                        [x + i, line_h - i],
                    ],
                    true,
                    1.5,
                    LineCap::Butt,
                    cursor.color,
                )
            }
        };
    }
    let el = if line.runs.len() == 1 {
        let r = &line.runs[0];
        let mut e = text(content)
            .font(TextStyle {
                weight: r.weight,
                italic: r.italic,
                ..TERM_FONT
            })
            .color(r.color);
        if r.underline.is_some() {
            e = e.spans(spans);
        }
        e
    } else {
        // `rich` wraps at the element's width; a terminal row may never
        // wrap — a wrapped row is two lines tall and pushes the grid's
        // bottom rows out of the pane. `text` stays one line; the spans
        // carry the per-run styling.
        text(content).font(TERM_FONT).color(ink.text).spans(spans)
    };
    // URLs and file paths open on Cmd-click (`State::open_terminal_link`).
    let mut el = el;
    for (range, id) in links {
        el = el.link(
            Link::new(range, id)
                .hover(LinkPaint::new().underline(ink.accent))
                .help("Cmd-click to open"),
        );
    }
    // A grid row is exactly one cell line, whatever the font says.
    // Rows select and copy as one group: drag across them, Cmd+C copies.
    el.shape(shape)
        .h(line_h)
        .selectable(true)
        .selection_color(ink.accent.alpha(0.35))
}

// ---------------------------------------------------------------------------
// Line elements — the rows a pane draws: text over cell backgrounds and the
// cursor, laid out on the monospace cell grid.
// ---------------------------------------------------------------------------

/// What an input method's commit types. Telex ends a word on space, but
/// the window swallows that key (only the commit arrives), so a committed
/// Latin word gets its space back. Other scripts (CJK candidates) are typed
/// as they are.
// ponytail: punctuation or Enter that commits a word is lost either way; a
// commit by Enter now leaves a trailing space.
pub fn committed_text(text: &str) -> String {
    let latin = |c: char| {
        c.is_ascii()
            || ('\u{00C0}'..='\u{024F}').contains(&c)
            || ('\u{0300}'..='\u{036F}').contains(&c)
            || ('\u{1E00}'..='\u{1EFF}').contains(&c)
    };
    let mut typed = text.to_string();
    if text.chars().all(latin) && text.chars().last().is_some_and(char::is_alphanumeric) {
        typed.push(' ');
    }
    typed
}

/// What a paste of `text` types: newlines become CR (what Enter sends), and a
/// program in bracketed-paste mode gets the text wrapped so it can tell a paste
/// from typing. An embedded end marker is dropped so a paste cannot escape.
pub fn pasted_text(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", ""))
    } else {
        text
    }
}

/// What a drop of `paths` types: each path with the shell's special
/// characters backslash-escaped (as Terminal.app does), joined by spaces
/// and followed by one.
pub fn dropped_paths_text(paths: &[std::path::PathBuf]) -> String {
    let mut text = String::new();
    for path in paths {
        for c in path.to_string_lossy().chars() {
            if " \t!\"#$&'()*;<>?[\\]^`{|}".contains(c) {
                text.push('\\');
            }
            text.push(c);
        }
        text.push(' ');
    }
    text
}

/// The monospace cell: one character's advance and the line height, both
/// in points, measured from the same font the lines are drawn with.
pub fn cell_metrics() -> (f32, f32) {
    let cell_w = rune::text::advance("M", TERM_FONT).max(1.0);
    let (_, line_h) = rune::text::measure("M", &[], TERM_FONT, None);
    (cell_w, line_h.max(1.0))
}

/// A pane's rendered rows as elements: the mono cell metrics every pane
/// shares, so each split's backgrounds and cursor line up with the cells
/// its grid was sized to.
pub fn term_lines_el(lines: &[TermLine], ink: &Ink, session: &str) -> Vec<El> {
    let (cell_w, line_h) = cell_metrics();
    lines
        .iter()
        .zip(row_links(lines, session))
        .map(|(l, links)| term_line_el(l, ink, cell_w, line_h, links))
        .collect()
}

/// Each row's links as (byte range in the row, link id).
fn row_links(lines: &[TermLine], session: &str) -> Vec<Vec<(std::ops::Range<usize>, String)>> {
    let mut links: Vec<Vec<(std::ops::Range<usize>, String)>> = vec![Vec::new(); lines.len()];
    // Links are found over a whole wrapped text — a URL the terminal broke
    // across rows is one link — then handed back to each row it touches.
    let mut first = 0;
    while first < lines.len() {
        let mut last = first;
        while lines[last].wrapped && last + 1 < lines.len() {
            last += 1;
        }
        let texts: Vec<String> = lines[first..=last].iter().map(TermLine::text).collect();
        let joined = texts.concat();
        let found: Vec<_> = find_links(&joined)
            .into_iter()
            .map(|(range, kind)| {
                let id = link_id(&kind, &joined[range.clone()], session);
                (range, id)
            })
            .collect();
        let mut offset = 0;
        for (row, text) in texts.iter().enumerate() {
            let rows = offset..offset + text.len();
            for (range, id) in &found {
                let (start, end) = (range.start.max(rows.start), range.end.min(rows.end));
                if start < end {
                    links[first + row].push((start - offset..end - offset, id.clone()));
                }
            }
            offset = rows.end;
        }
        first = last + 1;
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::Output;
    use rune::{KeyEvent, Mods};

    fn ink() -> Ink {
        Ink::new()
    }

    /// A terminal fed by a channel the test writes into.
    fn terminal() -> (Terminal, std::sync::mpsc::Sender<Output>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<Response, BridgeError>>();
        let session = SessionId::new();
        (Terminal::new(session, rx, ready_rx), tx)
    }

    /// The rows after the output settled: the repaint waits for the burst
    /// to end, exactly as the app does.
    fn settled_lines(terminal: &mut Terminal, ink: &Ink) -> Vec<TermLine> {
        while terminal.hold().is_some() {
            std::thread::sleep(Duration::from_millis(2));
        }
        terminal.lines(ink).to_vec()
    }

    /// The text of the first rendered row, without the grid's padding.
    fn first_line(terminal: &mut Terminal, ink: &Ink) -> String {
        settled_lines(terminal, ink)
            .first()
            .map(|line| {
                line.runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>()
            })
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    #[test]
    fn the_wheel_scrolls_the_history_and_typing_returns_to_the_bottom() {
        let (mut term, tx) = terminal();
        let data: String = (0..100).map(|n| format!("line{n}\r\n")).collect();
        tx.send(Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        term.feed();
        let live = first_line(&mut term, &ink());
        // Positive points reveal older lines; a part of a line waits.
        term.scroll(40.0, 16.0);
        let up = first_line(&mut term, &ink());
        assert_ne!(up, live, "the view moved into the history");
        // 40 points left half a line over: 4 more are still under a
        // line, 8 more make one.
        term.scroll(4.0, 16.0);
        assert_eq!(first_line(&mut term, &ink()), up, "part of a line waits");
        term.scroll(4.0, 16.0);
        assert_ne!(first_line(&mut term, &ink()), up, "the parts add up");
        term.scroll(-1000.0, 16.0);
        assert_eq!(
            first_line(&mut term, &ink()),
            live,
            "scrolled back to the bottom"
        );
        term.scroll(40.0, 16.0);
        term.scroll_to_bottom();
        assert_eq!(first_line(&mut term, &ink()), live);
    }

    #[test]
    fn a_committed_latin_word_gets_the_swallowed_space_back() {
        assert_eq!(committed_text("con"), "con ");
        assert_eq!(committed_text("việt"), "việt ");
        assert_eq!(committed_text("tốt"), "tốt ");
        // Decomposed marks stay one word.
        assert_eq!(
            committed_text("vie\u{0323}\u{302}t"),
            "vie\u{0323}\u{302}t "
        );
        // Other scripts and non-words are typed as they are.
        assert_eq!(committed_text("你好"), "你好");
        assert_eq!(committed_text(" "), " ");
        assert_eq!(committed_text(""), "");
    }

    #[test]
    fn urls_and_file_paths_are_found_in_a_row() {
        let f = |text: &str| -> Vec<(String, LinkKind)> {
            find_links(text)
                .into_iter()
                .map(|(r, k)| (text[r].to_string(), k))
                .collect()
        };
        let file = |path: &str, line| LinkKind::File {
            path: path.to_string(),
            line,
        };
        assert_eq!(
            f("see https://a.dev/x?y=1, and (https://b.dev/w_(v))."),
            vec![
                ("https://a.dev/x?y=1".to_string(), LinkKind::Url),
                ("https://b.dev/w_(v)".to_string(), LinkKind::Url),
            ]
        );
        assert_eq!(
            f("● Update(src/main.rs:42:7) and ./a/b.ts."),
            vec![
                (
                    "src/main.rs:42:7".to_string(),
                    file("src/main.rs", Some(42))
                ),
                ("./a/b.ts".to_string(), file("./a/b.ts", None)),
            ]
        );
        // No slash and no line, no extension, a URL's own path, a bare http.
        assert!(f("README.md node.js v1.2 a/b http://").is_empty());
        assert_eq!(f("x.rs:9 ~/k/n.md").len(), 2);
        assert_eq!(f("http://h/p/f.rs").len(), 1, "the URL claims its path");
    }

    #[test]
    fn a_url_the_terminal_wrapped_is_one_link_over_every_row() {
        let (mut term, tx) = terminal();
        term.resize(30, 6);
        let url = "https://example.dev/a/very/long/path/that/wraps?x=1";
        tx.send(Output {
            seq: 0,
            data: format!("see {url} ok\r\nnext\r\n").into_bytes(),
        })
        .unwrap();
        term.feed();
        let ink = ink();
        let lines = settled_lines(&mut term, &ink);
        assert!(lines[0].wrapped && !lines[1].wrapped, "one wrapped text");
        let ids = row_links(&lines, "s");
        assert_eq!(ids[0].len(), 1, "the first row carries the link");
        assert_eq!(ids[1].len(), 1, "so does the row it wraps onto");
        assert_eq!(ids[0][0].1, ids[1][0].1);
        assert_eq!(ids[0][0].1, format!("u\t{url}"), "the whole URL, not a cut");
        assert!(ids[2].is_empty());
    }

    #[test]
    fn a_paste_types_cr_newlines_and_brackets_only_on_request() {
        assert_eq!(pasted_text("a\r\nb\nc", false), "a\rb\rc");
        assert_eq!(pasted_text("x\ny", true), "\x1b[200~x\ry\x1b[201~");
        assert_eq!(pasted_text("a\x1b[201~b", true), "\x1b[200~ab\x1b[201~");
    }

    #[test]
    fn dropped_paths_are_escaped_for_the_shell() {
        let paths = [
            std::path::PathBuf::from("/tmp/a b.txt"),
            std::path::PathBuf::from("/tmp/it's(1).png"),
        ];
        assert_eq!(
            dropped_paths_text(&paths),
            "/tmp/a\\ b.txt /tmp/it\\'s\\(1\\).png "
        );
    }

    #[test]
    fn the_cursor_cell_follows_the_output_and_the_scroll() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: b"abc".to_vec(),
        })
        .unwrap();
        term.feed();
        assert_eq!(term.cursor_cell(), Some((0, 3)));
        let data: String = (0..100).map(|n| format!("line{n}\r\n")).collect();
        tx.send(Output {
            seq: 3,
            data: data.into_bytes(),
        })
        .unwrap();
        term.feed();
        let live = term.cursor_cell();
        assert!(live.is_some());
        term.scroll(1600.0, 16.0);
        assert_eq!(
            term.cursor_cell(),
            None,
            "the cursor scrolled off the screen"
        );
    }

    #[test]
    fn output_bytes_land_in_the_grid() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: b"hello\r\n".to_vec(),
        })
        .unwrap();
        assert!(term.feed());
        assert_eq!(first_line(&mut term, &ink()), "hello");
    }

    #[test]
    fn escape_sequences_colour_the_runs() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[31mred\u{1b}[0m\r\n".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let runs = &lines[0].runs;
        assert_eq!(runs[0].text, "red");
        // The red run is the Tango palette's red, not the default text.
        assert_ne!(runs[0].color, ink().text);
    }

    #[test]
    fn utf8_output_is_decoded() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "héllo\r\n".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        assert_eq!(first_line(&mut term, &ink()), "héllo");
    }

    #[test]
    fn the_next_seq_follows_the_bytes_that_arrived() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 10,
            data: b"abc".to_vec(),
        })
        .unwrap();
        term.feed();
        assert_eq!(term.next_seq, 13);
    }

    #[test]
    fn a_closed_reply_channel_does_not_drop_the_attachment() {
        // The daemon's answer arrives and the channel closes behind it: the
        // second read sees a closed channel, which is no answer and no
        // error. Treating it as an error dropped the attachment — and the
        // app re-attached in a loop, which read as a shaking terminal.
        let (_tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        ready_tx
            .send(Ok(Response {
                id: 1,
                outcome: protocol::message::ResponseOutcome::Result {
                    result: serde_json::json!({ "replay_from": 0, "next_seq": 10 }),
                },
            }))
            .unwrap();
        let mut term = Terminal::new(SessionId::new(), rx, ready_rx);

        let reply = term.subscribe_reply().expect("the answer is there");
        assert_eq!(reply.unwrap().next_seq, 10);
        // The channel is closed now: no answer, and no error either.
        assert!(term.subscribe_reply().is_none());
    }

    #[test]
    fn a_cursor_addressed_redraw_lands_where_it_says() {
        let (mut term, tx) = terminal();
        // What a full-screen program does: switch to the alternate screen,
        // clear it, and place text at known positions.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x1b[?1049h");
        bytes.extend_from_slice(b"\x1b[2J");
        bytes.extend_from_slice(b"\x1b[10;5Hbottom");
        bytes.extend_from_slice(b"\x1b[1;1Htop");
        tx.send(Output {
            seq: 0,
            data: bytes,
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let text_of = |index: usize| -> String {
            lines[index]
                .runs
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
        };
        assert!(
            text_of(0).starts_with("top"),
            "row 1 holds the top text: {:?}",
            text_of(0)
        );
        assert!(
            text_of(9).contains("bottom"),
            "row 10 holds the bottom text: {:?}",
            text_of(9)
        );
    }

    #[test]
    fn a_burst_of_output_is_drawn_once_it_settles() {
        let (mut term, tx) = terminal();
        // The grid is drawn once first: from then on a burst of output
        // waits for the output to settle.
        tx.send(Output {
            seq: 0,
            data: b"ready\r\n".to_vec(),
        })
        .unwrap();
        term.feed();
        assert!(!settled_lines(&mut term, &ink()).is_empty());

        // A full-screen redraw arrives in many pieces; `seq` is the byte
        // offset of each chunk's first byte, so it advances by chunk len.
        let mut seq = term.next_seq;
        for line in 0..8 {
            let data = format!("\u{1b}[Hline {line}\r\n").into_bytes();
            let len = data.len() as u64;
            tx.send(Output { seq, data }).unwrap();
            seq += len;
        }
        assert!(term.feed());
        // The repaint waits for the burst to end: the screen is drawn
        // once, not piece by piece.
        assert!(term.hold().is_some());
        std::thread::sleep(Duration::from_millis(25));
        assert!(term.hold().is_none());
        let lines = term.lines(&ink());
        assert!(
            lines
                .iter()
                .any(|line| line.runs.iter().any(|run| run.text.contains("line 7"))),
            "the whole burst landed: {lines:?}"
        );
    }

    #[test]
    fn a_resubscribe_replay_is_not_parsed_twice() {
        let (mut term, tx) = terminal();
        let bytes = b"hello\r\nworld\r\n".to_vec();
        tx.send(Output {
            seq: 0,
            data: bytes.clone(),
        })
        .unwrap();
        term.feed();
        let before = settled_lines(&mut term, &ink());
        assert!(
            before.iter().any(|line| line.text().contains("world")),
            "the first draw landed: {before:?}"
        );

        // The daemon replays its retained buffer on resubscribe: every
        // byte is behind the parser already and must be skipped.
        let (rtx, rrx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        term.resubscribe(rrx, ready_rx);
        rtx.send(Output {
            seq: 0,
            data: bytes,
        })
        .unwrap();
        assert!(!term.feed(), "the replayed bytes were already parsed");
        assert_eq!(term.lines(&ink()), &before[..]);
    }

    #[test]
    fn a_replay_overlapping_the_parsed_tail_is_trimmed() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: b"hello".to_vec(),
        })
        .unwrap();
        term.feed();
        assert_eq!(term.next_seq, 5);

        // The retained buffer starts before what was parsed: only the
        // tail of the chunk is new.
        let (rtx, rrx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        term.resubscribe(rrx, ready_rx);
        rtx.send(Output {
            seq: 2,
            data: b"llo world".to_vec(),
        })
        .unwrap();
        assert!(term.feed());
        assert_eq!(term.next_seq, 11);
        assert_eq!(first_line(&mut term, &ink()), "hello world");
    }

    #[test]
    fn the_subscribe_reply_does_not_move_the_ack_watermark() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut term = Terminal::new(SessionId::new(), rx, ready_rx);
        ready_tx
            .send(Ok(Response {
                id: 1,
                outcome: protocol::message::ResponseOutcome::Result {
                    result: serde_json::json!({ "replay_from": 30, "next_seq": 42 }),
                },
            }))
            .unwrap();
        let reply = term
            .subscribe_reply()
            .expect("the answer is there")
            .unwrap();
        assert_eq!(reply.next_seq, 42);
        // The replay has not arrived: nothing is acknowledged yet.
        assert_eq!(term.next_seq, 0);

        // The first replay may start at a nonzero offset: the gap is the
        // daemon's dropped head, the chunk itself is parsed whole.
        tx.send(Output {
            seq: 30,
            data: vec![b'x'; 12],
        })
        .unwrap();
        term.feed();
        assert_eq!(term.next_seq, 42);
    }

    #[test]
    fn a_continuous_burst_is_held_for_at_most_max_hold() {
        let (mut term, tx) = terminal();
        // Draw once so later output counts as a burst.
        tx.send(Output {
            seq: 0,
            data: b"ready\r\n".to_vec(),
        })
        .unwrap();
        term.feed();
        assert!(!settled_lines(&mut term, &ink()).is_empty());

        // New output opens a burst. Chunks keep arriving, so the quiet
        // window never opens — but the burst still ages past MAX_HOLD.
        tx.send(Output {
            seq: 7,
            data: b"x".to_vec(),
        })
        .unwrap();
        term.feed();
        assert!(term.hold().is_some());
        term.last = Instant::now();
        term.burst = Some(Instant::now() - MAX_HOLD - Duration::from_millis(1));
        assert!(term.hold().is_none(), "MAX_HOLD bounds a busy stream");

        // The forced draw forgets the burst; the next chunk opens a new
        // one that is held again.
        term.lines(&ink());
        assert!(term.burst.is_none());
        tx.send(Output {
            seq: 8,
            data: b"y".to_vec(),
        })
        .unwrap();
        term.feed();
        assert!(term.burst.is_some());
        assert!(term.hold().is_some());
    }

    #[test]
    fn a_resized_grid_renders_the_new_shape() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: b"hello\r\n".to_vec(),
        })
        .unwrap();
        term.feed();
        assert_eq!(term.size(), (TERM_COLS, TERM_ROWS));

        // The pane got smaller: the grid follows it, and the content
        // reflows into the new shape.
        term.resize(40, 6);
        assert_eq!(term.size(), (40, 6));
        let lines = settled_lines(&mut term, &ink());
        assert_eq!(lines.len(), 6);
        assert!(
            lines
                .iter()
                .any(|line| line.runs.iter().any(|run| run.text.contains("hello"))),
            "the text survived the resize"
        );
    }

    #[test]
    fn ansi_backgrounds_become_cell_spans() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[41mRED\u{1b}[42mGRN\u{1b}[0m plain\r\n"
                .as_bytes()
                .to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let backs = &lines[0].backs;
        assert_eq!(backs.len(), 2, "one span per colour run: {backs:?}");
        assert_eq!((backs[0].start, backs[0].end), (0, 3));
        assert_eq!(backs[0].color, ansi16(1));
        assert_eq!((backs[1].start, backs[1].end), (3, 6));
        assert_eq!(backs[1].color, ansi16(2));
        // " plain" has the default background: no span covers it.
        assert!(backs.iter().all(|b| b.end <= 6));
    }

    #[test]
    fn background_spans_merge_across_style_runs() {
        let (mut term, tx) = terminal();
        // The fg changes mid-span; the bg does not, so one span covers all
        // three cells instead of one per styled run.
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[41mRE\u{1b}[31mD\u{1b}[0m\r\n".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        assert_eq!(lines[0].backs.len(), 1, "{:?}", lines[0].backs);
        assert_eq!((lines[0].backs[0].start, lines[0].backs[0].end), (0, 3));
    }

    #[test]
    fn inverse_video_swaps_the_cell_colours() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[7mINV\u{1b}[0m\r\n".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let ink = ink();
        // SGR 7 swaps fg/bg: the cells' background becomes the text ink
        // and their ink the pane background.
        assert_eq!(lines[0].backs.len(), 1, "{:?}", lines[0].backs);
        assert_eq!(lines[0].backs[0].color, ink.text);
        assert_eq!(lines[0].runs[0].text.trim_end(), "INV");
        assert_eq!(lines[0].runs[0].color, ink.bg);
    }

    #[test]
    fn the_cursor_is_where_the_term_puts_it() {
        let (mut term, tx) = terminal();
        // An underline cursor so the marker is inspectable; the block
        // shape is covered by the inverse-swap test below.
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[3 qabc\u{1b}[5;10H".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let cursor = lines[4].cursor.as_ref().expect("the cursor sits on row 5");
        assert_eq!((cursor.col, cursor.span), (9, 1));
        assert_eq!(cursor.shape, CursorShape::Underline);
        // The cell's own ink colours the cursor.
        assert_eq!(cursor.color, ink().text);
        // Every other row has no cursor.
        assert!(
            lines
                .iter()
                .enumerate()
                .all(|(i, l)| i == 4 || l.cursor.is_none())
        );

        // ?25l hides it: nothing is painted, not even a marker.
        tx.send(Output {
            seq: 15,
            data: "\u{1b}[?25l".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        assert!(lines.iter().all(|l| l.cursor.is_none()));
    }

    #[test]
    fn a_block_cursor_is_the_cell_drawn_inverse() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "ab".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let ink = ink();
        // The cursor sits on cell 2: its background span takes the cell's
        // ink and its glyph the pane background.
        let back = lines[0]
            .backs
            .iter()
            .find(|b| b.start == 2 && b.end == 3)
            .expect("the cursor cell is a background span");
        assert_eq!(back.color, ink.text);
        // No marker: the block shape paints through the cell swap.
        assert!(lines[0].cursor.is_none());
    }

    #[test]
    fn a_palette_swap_repaints_the_cached_rows() {
        let (mut term, tx) = terminal();
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[41mRED\u{1b}[0m".as_bytes().to_vec(),
        })
        .unwrap();
        term.feed();
        let dark = ink();
        let before = settled_lines(&mut term, &dark);
        let light = Ink::for_theme(crate::theme::Theme::Light);
        // Without a theme refresh the cached rows keep the old ink.
        {
            let stale = term.lines(&light);
            assert_eq!(stale[0].runs[0].color, dark.text);
        }
        term.refresh_theme();
        let after = settled_lines(&mut term, &light);
        assert_eq!(after[0].text(), before[0].text(), "same grid, new ink");
        // The resolved fg followed the palette; the ANSI bg did not.
        assert_eq!(after[0].runs[0].color, light.text);
        assert_eq!(after[0].backs[0].color, ansi16(1));
        // The stream offsets and the parser did not move.
        assert_eq!(term.next_seq, 12);
    }

    #[test]
    fn wide_chars_and_combining_marks_keep_their_cells() {
        let (mut term, tx) = terminal();
        //  is a wide glyph (two cells), ́ a combining mark on 'e'.
        tx.send(Output {
            seq: 0,
            data: "\u{1b}[3 qa\u{4e16}b e\u{301}\u{1b}[1;2H"
                .as_bytes()
                .to_vec(),
        })
        .unwrap();
        term.feed();
        let lines = settled_lines(&mut term, &ink());
        let text = lines[0].text();
        assert!(
            text.starts_with("a\u{4e16}b e\u{301}"),
            "the wide glyph emits once and the mark stays on its base: {text:?}"
        );
        // No spacer cell became a space between  and b.
        assert!(!text.contains("\u{4e16} "), "{text:?}");
        // The cursor was addressed onto the wide glyph: it covers both
        // of the glyph's cells.
        let cursor = lines[0].cursor.as_ref().expect("cursor on the glyph");
        assert_eq!((cursor.col, cursor.span), (1, 2));
    }

    #[test]
    fn keys_encode_to_the_bytes_a_terminal_expects() {
        let text = |key: rune::Key| {
            let bytes = key_bytes(&KeyEvent {
                key,
                mods: Mods::default(),
            })
            .unwrap();
            String::from_utf8(bytes).unwrap()
        };
        assert_eq!(text(rune::Key::Text("a".into())), "a");
        assert_eq!(text(rune::Key::Text("é".into())), "é");
        assert_eq!(text(rune::Key::Enter), "\r");
        assert_eq!(text(rune::Key::Backspace), "\u{7f}");
        assert_eq!(text(rune::Key::Tab), "\t");
        assert_eq!(text(rune::Key::Escape), "\u{1b}");
        assert_eq!(text(rune::Key::Left), "\u{1b}[D");
        assert_eq!(text(rune::Key::Right), "\u{1b}[C");
        assert_eq!(text(rune::Key::Up), "\u{1b}[A");
        assert_eq!(text(rune::Key::Down), "\u{1b}[B");
        assert_eq!(text(rune::Key::Home), "\u{1b}[H");
        assert_eq!(text(rune::Key::End), "\u{1b}[F");
        assert_eq!(text(rune::Key::PageUp), "\u{1b}[5~");
        assert_eq!(text(rune::Key::PageDown), "\u{1b}[6~");
        // The keys a terminal has no use for are not sent.
        assert!(
            key_bytes(&KeyEvent {
                key: rune::Key::Function(12),
                mods: Mods::default(),
            })
            .is_none()
        );
    }

    #[test]
    fn shift_or_option_enter_is_a_newline_not_a_submit() {
        let enter = |mods: Mods| {
            key_bytes(&KeyEvent {
                key: rune::Key::Enter,
                mods,
            })
            .unwrap()
        };
        assert_eq!(enter(Mods::default()), b"\r");
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        assert_eq!(enter(shift), b"\x1b\r");
        assert_eq!(enter(alt), b"\x1b\r");
    }

    #[test]
    fn modified_keys_encode_like_a_mac_terminal() {
        let send = |key: rune::Key, shift, alt, ctrl, cmd| {
            let mods = Mods {
                shift,
                alt,
                ctrl,
                cmd,
                logo: cmd,
            };
            key_bytes(&KeyEvent { key, mods }).map(|b| String::from_utf8(b).unwrap())
        };
        use rune::Key::*;
        // Shift+Tab is back-tab.
        assert_eq!(send(Tab, true, false, false, false).unwrap(), "\u{1b}[Z");
        // ⌘←/⌘→/⌘⌫ are line start / end / kill; other ⌘ keys stay the app's.
        assert_eq!(send(Left, false, false, false, true).unwrap(), "\u{1}");
        assert_eq!(send(Right, false, false, false, true).unwrap(), "\u{5}");
        assert_eq!(
            send(Backspace, false, false, false, true).unwrap(),
            "\u{15}"
        );
        assert_eq!(send(Up, false, false, false, true), None);
        assert_eq!(send(Command('c'), false, false, false, true), None);
        // ⌥←/⌥→/⌥⌫ are word moves and word delete.
        assert_eq!(send(Left, false, true, false, false).unwrap(), "\u{1b}b");
        assert_eq!(send(Right, false, true, false, false).unwrap(), "\u{1b}f");
        assert_eq!(
            send(Backspace, false, true, false, false).unwrap(),
            "\u{1b}\u{7f}"
        );
        // Other modified navigation keys carry xterm's modifier parameter.
        assert_eq!(
            send(Right, true, false, false, false).unwrap(),
            "\u{1b}[1;2C"
        );
        assert_eq!(
            send(Left, false, false, true, false).unwrap(),
            "\u{1b}[1;5D"
        );
        assert_eq!(
            send(Delete, false, false, true, false).unwrap(),
            "\u{1b}[3;5~"
        );
        // Ctrl+punctuation control bytes.
        assert_eq!(
            send(Text("[".into()), false, false, true, false).unwrap(),
            "\u{1b}"
        );
        assert_eq!(
            send(Text(" ".into()), false, false, true, false).unwrap(),
            "\0"
        );
    }

    #[test]
    fn ctrl_letters_are_control_bytes() {
        let bytes = key_bytes(&KeyEvent {
            key: rune::Key::Text("c".into()),
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
        })
        .unwrap();
        assert_eq!(bytes, vec![0x03]);
        // Shift+Ctrl is still the control byte.
        let bytes = key_bytes(&KeyEvent {
            key: rune::Key::Text("C".into()),
            mods: Mods {
                ctrl: true,
                shift: true,
                ..Mods::default()
            },
        })
        .unwrap();
        assert_eq!(bytes, vec![0x03]);
    }
}

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
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use app_core::error::{BridgeError, Code};
use protocol::message::Response;
use protocol::session::SessionId;
use rune::{Color, El, Span, TextStyle, Tx, col, el, rich, text};

use crate::State;
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
}

/// One terminal row: styled runs of text, left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct TermLine {
    runs: Vec<TermRun>,
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
    /// The next byte the daemon will send — the flow-control watermark.
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
        if let Some(Ok(subscribe)) = reply
            && subscribe.next_seq > self.next_seq
        {
            // The stream continues after the replay: the acknowledgement
            // watermark starts there.
            self.next_seq = subscribe.next_seq;
        }
        reply
    }
    /// Feed the bytes the daemon sent. Returns whether the grid changed.
    pub fn feed(&mut self) -> bool {
        let mut changed = false;
        while let Ok(chunk) = self.chunks.try_recv() {
            self.parser.advance(&mut self.term, &chunk.data);
            self.next_seq = chunk.seq + chunk.data.len() as u64;
            self.last = Instant::now();
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// How long the repaint may be held back: the output has to settle
    /// first, so a full-screen program is drawn once instead of line by
    /// line. `None` means "repaint now".
    pub fn hold(&self) -> Option<Duration> {
        if !self.dirty || !self.rendered {
            return None;
        }
        let since = self.last.elapsed();
        if since >= MAX_HOLD || since >= QUIET {
            return None;
        }
        Some(QUIET - since)
    }

    /// The rendered rows of the visible grid, repainted when the output
    /// settled.
    pub fn lines(&mut self, ink: &Ink) -> &[TermLine] {
        if self.dirty && self.hold().is_none() {
            self.lines = render_lines(&self.term, ink);
            self.dirty = false;
            self.rendered = true;
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
        // A resized grid is drawn at once: no output is settling.
        self.last = Instant::now()
            .checked_sub(QUIET)
            .unwrap_or_else(Instant::now);
    }

    /// The grid size the terminal renders at.
    pub fn size(&self) -> (usize, usize) {
        (self.term.grid().columns(), self.term.grid().screen_lines())
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

/// A believable colored shell session as a PTY byte stream: SGR colour,
/// bold/dim, 256-colour and truecolour escapes, \r\n line ends. Only the
/// tests read it — the app renders the daemon's live stream.
#[cfg(test)]
const ANSI_TRANSCRIPT: &str = "\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m cargo test -p gokod\r\n\
   \u{1b}[2mCompiling\u{1b}[0m gokod v0.4.1 (/home/user/Project/goko/crates/gokod)\r\n\
    \u{1b}[2mFinished\u{1b}[0m `test` profile [unoptimized + debuginfo] target(s) in 4.21s\r\n\
     \u{1b}[2mRunning\u{1b}[0m unittests src/lib.rs\r\n\
\r\n\
running 42 tests\r\n\
test pty::spawn_with_size ... \u{1b}[32mok\u{1b}[0m\r\n\
test proto::frame_roundtrip ... \u{1b}[32mok\u{1b}[0m\r\n\
test session::attach_detach ... \u{1b}[32mok\u{1b}[0m\r\n\
test scrollback::resize_reflow ... \u{1b}[32mok\u{1b}[0m\r\n\
\r\n\
\u{1b}[1;32mtest result: ok. 42 passed\u{1b}[0m; 0 failed; 0 ignored\r\n\
\r\n\
\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m git status\r\n\
On branch main\r\n\
Your branch is up to date with 'origin/main'.\r\n\
\u{1b}[2mnothing to commit, working tree clean\u{1b}[0m\r\n\
\r\n\
\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m ./gokod --bad-flag\r\n\
\u{1b}[31merror\u{1b}[0m: unexpected argument '--bad-flag' found\r\n\
\r\n\
  \u{1b}[2mtip: a similar argument exists: '--flags'\u{1b}[0m\r\n\
\r\n\
\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m ls -la target/\r\n\
\u{1b}[34mdrwxr-xr-x\u{1b}[0m  2 user user  4096 Nov  8 18:51 .\r\n\
\u{1b}[34mdrwxr-xr-x\u{1b}[0m 14 user user  4096 Nov  8 19:10 ..\r\n\
\u{1b}[32m-rwxr-xr-x\u{1b}[0m  1 user user   12M Nov  8 19:10 goko-rune\r\n\
\u{1b}[38;5;208m-rw-r--r--\u{1b}[0m  1 user user   24K Nov  8 18:58 ui.png\r\n\
\u{1b}[38;2;180;142;255m-rw-r--r--\u{1b}[0m  1 user user   98K Nov  8 19:10 tree.json\r\n\
\r\n\
\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m \u{1b}[1mcargo build --release\u{1b}[0m\r\n\
   \u{1b}[2mCompiling\u{1b}[0m rune v0.1.0\r\n\
    \u{1b}[2mFinished\u{1b}[0m `release` profile [optimized] target(s) in 38.04s\r\n\
\r\n\
\u{1b}[1;32muser@devbox\u{1b}[0m \u{1b}[34m~/Project/goko\u{1b}[0m \u{1b}[2m%\u{1b}[0m \u{1b}[7m█\u{1b}[0m";

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
        A::Spec(rgb) => Color {
            r: rune::color::srgb_decode(rgb.r as f32 / 255.0),
            g: rune::color::srgb_decode(rgb.g as f32 / 255.0),
            b: rune::color::srgb_decode(rgb.b as f32 / 255.0),
            a: 1.0,
        },
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
    let rows = term.grid().screen_lines();
    let cols = term.grid().columns();
    let mut grid: Vec<Vec<&Cell>> = (0..rows).map(|_| Vec::with_capacity(cols)).collect();
    for indexed in term.grid().display_iter() {
        let line = indexed.point.line.0;
        if (0..rows as i32).contains(&line) {
            grid[line as usize].push(indexed.cell);
        }
    }

    grid.into_iter()
        .map(|row| {
            let mut runs: Vec<TermRun> = Vec::new();
            let mut cur = String::new();
            let mut cur_style: Option<(Color, f32, bool)> = None;
            for cell in row {
                if cell.flags.contains(Flags::HIDDEN) {
                    cur.push(' ');
                    continue;
                }
                let mut fg = ansi_color(cell.fg, ink);
                if cell.flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
                    // xterm.js draws DIM as the colour at 50% opacity over the
                    // background (.xterm-dim { color: multiplyOpacity(.5) }).
                    fg = fg.mix(ink.bg, 0.5);
                }
                // INVERSE video and per-cell backgrounds aren't representable —
                // the trailing SGR-7 block cursor still reads as a light block.
                let weight = if cell.flags.intersects(Flags::BOLD | Flags::BOLD_ITALIC) {
                    700.0
                } else {
                    400.0
                };
                let italic = cell.flags.contains(Flags::ITALIC);
                let style = (fg, weight, italic);
                if cur_style != Some(style) {
                    if !cur.is_empty() {
                        let (color, weight, italic) = cur_style.unwrap();
                        runs.push(TermRun {
                            text: std::mem::take(&mut cur),
                            color,
                            weight,
                            italic,
                        });
                    }
                    cur_style = Some(style);
                }
                cur.push(cell.c);
            }
            if !cur.is_empty() {
                let (color, weight, italic) = cur_style.unwrap();
                runs.push(TermRun {
                    text: cur,
                    color,
                    weight,
                    italic,
                });
            }
            if runs.is_empty() {
                runs.push(TermRun {
                    text: " ".into(),
                    color: ink.text,
                    weight: 400.0,
                    italic: false,
                });
            }
            TermLine { runs }
        })
        .collect()
}

/// The bytes a key press sends to the PTY: the ANSI sequences a terminal
/// expects. `None` for the keys a terminal has no use for (function keys,
/// the modifier keys themselves).
pub fn key_bytes(ev: &rune::KeyEvent) -> Option<Vec<u8>> {
    use rune::Key;
    match &ev.key {
        Key::Text(text) => {
            // Ctrl+letter is a control byte; everything else is its UTF-8.
            if ev.mods.ctrl
                && !ev.mods.cmd
                && let Some(ch) = text.chars().next()
                && text.chars().count() == 1
                && ch.is_ascii_alphabetic()
            {
                return Some(vec![(ch.to_ascii_lowercase() as u8) & 0x1f]);
            }
            Some(text.as_bytes().to_vec())
        }
        Key::Enter => Some(vec![b'\r']),
        Key::Backspace => Some(vec![0x7f]),
        Key::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
        Key::Tab => Some(vec![b'\t']),
        Key::Escape => Some(vec![0x1b]),
        Key::Left => Some(vec![0x1b, b'[', b'D']),
        Key::Right => Some(vec![0x1b, b'[', b'C']),
        Key::Up => Some(vec![0x1b, b'[', b'A']),
        Key::Down => Some(vec![0x1b, b'[', b'B']),
        Key::Home => Some(vec![0x1b, b'[', b'H']),
        Key::End => Some(vec![0x1b, b'[', b'F']),
        Key::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
        Key::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
        Key::Insert => Some(vec![0x1b, b'[', b'2', b'~']),
        _ => None,
    }
}

/// 13 pt monospace on a 16 pt line — xterm fontSize 13 with a ~16 px cell.
const TERM_FONT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(16.0);

fn term_line_el(line: &TermLine, ink: &Ink) -> El {
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
        spans.push(sp);
        content.push_str(&run.text);
        at = end;
    }
    if line.runs.len() == 1 {
        let r = &line.runs[0];
        text(content)
            .font(TextStyle {
                weight: r.weight,
                italic: r.italic,
                ..TERM_FONT
            })
            .color(r.color)
    } else {
        rich(content, spans).font(TERM_FONT).color(ink.text)
    }
}

// ---------------------------------------------------------------------------
// Terminal area — bg-bg, focused pane ring-1 ring-inset ring-accent/40, xterm
// host inset-2 (8 px) around the line grid.
// ---------------------------------------------------------------------------

/// The pane's inner padding — the host inset around the line grid.
const PAD: f32 = 8.0;

/// The terminal area: the attached session's live PTY grid.
///
/// The grid follows the pane: it is measured from the pane's own rect, so
/// the session draws exactly what fits. A grid wider than the pane would
/// wrap its lines — and a wrapped line makes the view taller, which
/// scrolls, which moves the content: a full-screen program would shake.
/// The grid is the viewport here (no scroll view), the way a terminal
/// shows the screen it draws on.
pub fn terminal_area(s: &State, ink: &Ink) -> El {
    // The monospace cell: one character's advance and the line height, both
    // in points, measured from the same font the lines are drawn with.
    let cell_w = rune::text::advance("M", TERM_FONT).max(1.0);
    let (_, line_h) = rune::text::measure("M", &[], TERM_FONT, None);
    let line_h = line_h.max(1.0);
    el().grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .w_full()
        .fill(ink.bg)
        // ring-inset ring-accent/40: a 1 px accent line just inside the pane edge.
        .border(1.0, ink.accent.alpha(0.4))
        .on_layout_tx(move |s: &mut State, rect: [f32; 4], tx: &mut Tx| {
            // A point to spare on the width: a line that is a hair too
            // wide would wrap, and a wrapped line is what makes the
            // content jump.
            let cols = ((rect[2] - 2.0 * PAD - 1.0) / cell_w).floor().max(20.0) as usize;
            let rows = ((rect[3] - 2.0 * PAD) / line_h).floor().max(4.0) as usize;
            if (cols, rows) != (s.terminal_cols, s.terminal_rows) {
                s.terminal_cols = cols;
                s.terminal_rows = rows;
                s.resize_terminal_grid();
                crate::trace::trace(&format!("layout rect={rect:?} grid={cols}x{rows}"));
                // The grid changed: its rows are drawn again on the next
                // frame.
                tx.wake_after(std::time::Duration::from_millis(16));
            }
        })
        .child(
            col()
                .w_full()
                .h_full()
                .min_h(0.0)
                .min_w(0.0)
                .padding(PAD)
                .shrink()
                .children(s.lines.iter().map(|l| term_line_el(l, ink))),
        )
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

        // A full-screen redraw arrives in many pieces.
        for line in 0..8 {
            tx.send(Output {
                seq: line + 1,
                data: format!("\u{1b}[Hline {line}\r\n").into_bytes(),
            })
            .unwrap();
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

#[cfg(test)]
mod scratch_capture {
    use super::*;
    use crate::daemon::Output;

    #[test]
    fn dump_capture() {
        let Some(dir) = std::env::var_os("CAP_DIR") else {
            return;
        };
        for name in ["claude", "codex"] {
            let data =
                std::fs::read(std::path::Path::new(&dir).join(format!("{name}.bin"))).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let (_r, ready) = std::sync::mpsc::channel::<Result<Response, BridgeError>>();
            let mut term = Terminal::new(SessionId::new(), rx, ready);
            // feed in 1 KiB chunks like the daemon would
            for (i, c) in data.chunks(1024).enumerate() {
                tx.send(Output {
                    seq: (i * 1024) as u64,
                    data: c.to_vec(),
                })
                .unwrap();
            }
            term.feed();
            std::thread::sleep(Duration::from_millis(30));
            let ink = Ink::new();
            let lines = term.lines(&ink).to_vec();
            println!("===== {name}: {} lines", lines.len());
            for (i, l) in lines.iter().enumerate() {
                let t = l.text();
                if !t.trim().is_empty() {
                    println!("{i:2}: {}", t.trim_end());
                }
            }
        }
    }
}

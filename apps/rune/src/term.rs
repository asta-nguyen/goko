//! Terminal stub: ANSI bytes -> alacritty_terminal grid -> styled rich-text
//! runs -> a scrolling line column. Ported verbatim from the verified
//! rune-spike `fidelity.rs` (itself from its `main.rs`), including the Tango
//! ansi16 palette the real TerminalController renders on #1e1e1e.

use rune::{Color, El, ScrollSpec, Span, TextStyle, col, el, rich, text};

use crate::State;
use crate::theme::Ink;

// ---------------------------------------------------------------------------
// Terminal: ANSI bytes -> alacritty_terminal grid -> styled runs.
// ---------------------------------------------------------------------------

/// One styled run of terminal text.
struct TermRun {
    text: String,
    color: Color,
    weight: f32,
    italic: bool,
}

/// One terminal row: styled runs of text, left to right.
pub struct TermLine {
    runs: Vec<TermRun>,
}

/// A believable colored shell session as a PTY byte stream: SGR colour,
/// bold/dim, 256-colour and truecolour escapes, \r\n line ends.
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

const TERM_COLS: usize = 120;
const TERM_ROWS: usize = 42;

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

/// Feeds `ANSI_TRANSCRIPT` through alacritty's `Term` and turns each visible
/// grid row into a `TermLine`.
pub fn real_term_lines(ink: &Ink) -> Vec<TermLine> {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::cell::{Cell, Flags};
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct Size;
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            TERM_ROWS
        }
        fn screen_lines(&self) -> usize {
            TERM_ROWS
        }
        fn columns(&self) -> usize {
            TERM_COLS
        }
    }

    let mut term = Term::new(Config::default(), &Size, VoidListener);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, ANSI_TRANSCRIPT.as_bytes());

    let mut rows: Vec<Vec<&Cell>> = (0..TERM_ROWS)
        .map(|_| Vec::with_capacity(TERM_COLS))
        .collect();
    for indexed in term.grid().display_iter() {
        let line = indexed.point.line.0;
        if (0..TERM_ROWS as i32).contains(&line) {
            rows[line as usize].push(indexed.cell);
        }
    }

    rows.into_iter()
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

pub fn terminal_area(s: &State, ink: &Ink) -> El {
    el().grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .w_full()
        .fill(ink.bg)
        // ring-inset ring-accent/40: a 1 px accent line just inside the pane edge.
        .border(1.0, ink.accent.alpha(0.4))
        .child(
            col()
                .w_full()
                .h_full()
                .min_h(0.0)
                .padding(8.0)
                .scroll(ScrollSpec { stick_bottom: true })
                .children(s.lines.iter().map(|l| term_line_el(l, ink)))
                .child(el().h(4.0)),
        )
}

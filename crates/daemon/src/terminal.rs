//! The daemon's copy of a session's terminal: an `alacritty_terminal` grid fed
//! every output chunk from the moment the process starts, so a GUI that attaches
//! later can be handed a first paint instead of the last megabyte of raw bytes.

use std::fmt::Write as _;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, Processor};

struct Size {
    columns: usize,
    screen_lines: usize,
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

/// Modes a program can switch on that the GUI's own parser has to hold again,
/// with the DEC private mode number that sets each.
const PRIVATE_MODES: [(TermMode, u16); 9] = [
    (TermMode::APP_CURSOR, 1),
    (TermMode::MOUSE_REPORT_CLICK, 1000),
    (TermMode::MOUSE_DRAG, 1002),
    (TermMode::MOUSE_MOTION, 1003),
    (TermMode::FOCUS_IN_OUT, 1004),
    (TermMode::UTF8_MOUSE, 1005),
    (TermMode::SGR_MOUSE, 1006),
    (TermMode::ALTERNATE_SCROLL, 1007),
    (TermMode::BRACKETED_PASTE, 2004),
];

/// The flags that change how a cell is drawn, with the SGR parameter for each.
const STYLE_FLAGS: [(Flags, &str); 10] = [
    (Flags::BOLD, "1"),
    (Flags::DIM, "2"),
    (Flags::ITALIC, "3"),
    (Flags::UNDERLINE, "4"),
    (Flags::DOUBLE_UNDERLINE, "4:2"),
    (Flags::UNDERCURL, "4:3"),
    (Flags::DOTTED_UNDERLINE, "4:4"),
    (Flags::DASHED_UNDERLINE, "4:5"),
    (Flags::INVERSE, "7"),
    (Flags::HIDDEN, "8"),
];

pub struct TerminalModel {
    term: Term<VoidListener>,
    parser: Processor,
}

impl TerminalModel {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = Size {
            columns: usize::from(cols),
            screen_lines: usize::from(rows),
        };
        Self {
            term: Term::new(Config::default(), &size, VoidListener),
            parser: Processor::new(),
        }
    }

    /// Parse output. The parser keeps its state between calls, so a sequence
    /// split across two chunks is still one sequence.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size {
            columns: usize::from(cols),
            screen_lines: usize::from(rows),
        });
    }

    /// ANSI bytes that rebuild this terminal in a fresh parser of the same
    /// size: the retained history, the screen, the cursor and the modes.
    // ponytail: the primary screen under a running alternate-screen program,
    // the scroll region, tab stops, charsets, hyperlinks, kitty keyboard flags
    // and a pending autowrap are not restored; the program's next redraw sets
    // what it needs. Add them when a reattach visibly breaks on one.
    pub fn paint(&mut self) -> Vec<u8> {
        // The parser holds the bytes of a synchronized update (CSI ?2026h) back
        // until its end; a paint taken mid-frame must still show them, and the
        // rest of the frame arrives on the live stream.
        self.parser.stop_sync(&mut self.term);
        let grid = self.term.grid();
        let mode = *self.term.mode();
        let (rows, cols) = (grid.screen_lines(), grid.columns());
        let mut out = String::from("\x1bc");
        let mut style = Style::default();

        if mode.contains(TermMode::ALT_SCREEN) {
            out.push_str("\x1b[?1049h");
        } else if grid.history_size() > 0 {
            // Written from the top, the rows overflow the screen into history;
            // clearing the screen then pushes the rows still on it after them.
            for back in (1..=grid.history_size()).rev() {
                let row = &grid[Line(-(back as i32))];
                let wrapped = write_row(&mut out, &mut style, row_cells(row, cols));
                if !wrapped {
                    out.push_str("\r\n");
                }
            }
            style.reset(&mut out);
            out.push_str("\x1b[2J");
        }

        // A row that wraps is followed by its continuation without a move, so
        // the parser sets the wrap flag itself.
        let mut continues = false;
        for line in 0..rows {
            if !continues {
                let _ = write!(out, "\x1b[{};1H", line + 1);
            }
            continues = write_row(
                &mut out,
                &mut style,
                row_cells(&grid[Line(line as i32)], cols),
            );
        }
        style.reset(&mut out);

        // DECOM homes the cursor, so the modes go before the final move.
        for (flag, number) in PRIVATE_MODES {
            if mode.contains(flag) {
                let _ = write!(out, "\x1b[?{number}h");
            }
        }
        if mode.contains(TermMode::ORIGIN) {
            out.push_str("\x1b[?6h");
        }
        if !mode.contains(TermMode::LINE_WRAP) {
            out.push_str("\x1b[?7l");
        }
        if mode.contains(TermMode::INSERT) {
            out.push_str("\x1b[4h");
        }
        if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
            out.push_str("\x1b[20h");
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            out.push_str("\x1b=");
        }
        let cursor = self.term.cursor_style();
        let shape = match (cursor.shape, cursor.blinking) {
            (CursorShape::Block, true) => Some(1),
            (CursorShape::Block, false) => Some(2),
            (CursorShape::Underline, true) => Some(3),
            (CursorShape::Underline, false) => Some(4),
            (CursorShape::Beam, true) => Some(5),
            (CursorShape::Beam, false) => Some(6),
            _ => None,
        };
        if let Some(shape) = shape {
            let _ = write!(out, "\x1b[{shape} q");
        }
        let point = grid.cursor.point;
        let _ = write!(out, "\x1b[{};{}H", point.line.0 + 1, point.column.0 + 1);
        out.push_str(if mode.contains(TermMode::SHOW_CURSOR) {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });
        out.into_bytes()
    }

    #[cfg(test)]
    fn term(&self) -> &Term<VoidListener> {
        &self.term
    }
}

fn row_cells(row: &alacritty_terminal::grid::Row<Cell>, cols: usize) -> Vec<&Cell> {
    (0..cols).map(|column| &row[Column(column)]).collect()
}

/// The visual style SGR has set so far; a change re-sets it from scratch.
#[derive(PartialEq)]
struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    underline: Option<Color>,
    flags: Flags,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: None,
            bg: None,
            underline: None,
            flags: Flags::empty(),
        }
    }
}

impl Style {
    fn of(cell: &Cell) -> Self {
        let flags = cell.flags
            & (Flags::BOLD
                | Flags::DIM
                | Flags::ITALIC
                | Flags::ALL_UNDERLINES
                | Flags::INVERSE
                | Flags::HIDDEN
                | Flags::STRIKEOUT);
        Self {
            fg: explicit(cell.fg),
            bg: explicit(cell.bg),
            underline: cell.underline_color().and_then(explicit),
            flags,
        }
    }

    fn write(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (flag, parameter) in STYLE_FLAGS {
            if self.flags.contains(flag) {
                let _ = write!(out, ";{parameter}");
            }
        }
        if self.flags.contains(Flags::STRIKEOUT) {
            out.push_str(";9");
        }
        for (base, color) in [(38, self.fg), (48, self.bg), (58, self.underline)] {
            match color {
                Some(Color::Indexed(index)) => {
                    let _ = write!(out, ";{base};5;{index}");
                }
                Some(Color::Spec(rgb)) => {
                    let _ = write!(out, ";{base};2;{};{};{}", rgb.r, rgb.g, rgb.b);
                }
                Some(Color::Named(named)) => {
                    // 0-7 are 30-37 / 40-47, 8-15 are 90-97 / 100-107; there is
                    // no named underline colour, so 58 falls back to indexed.
                    let n = named as usize;
                    let _ = match (base, n) {
                        (38, 0..=7) => write!(out, ";{}", 30 + n),
                        (38, _) => write!(out, ";{}", 82 + n),
                        (48, 0..=7) => write!(out, ";{}", 40 + n),
                        (48, _) => write!(out, ";{}", 92 + n),
                        _ => write!(out, ";{base};5;{n}"),
                    };
                }
                None => {}
            }
        }
        out.push('m');
    }

    fn reset(&mut self, out: &mut String) {
        if *self != Style::default() {
            out.push_str("\x1b[0m");
            *self = Style::default();
        }
    }
}

/// A colour a program chose, or `None` for the terminal's default.
fn explicit(color: Color) -> Option<Color> {
    match color {
        Color::Named(named) if (named as usize) >= 16 => None,
        color => Some(color),
    }
}

/// A cell that draws as nothing, so the end of a row can be left unwritten.
fn is_blank(cell: &Cell) -> bool {
    cell.c == ' '
        && explicit(cell.bg).is_none()
        && cell.extra.is_none()
        && !cell
            .flags
            .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
}

/// Write one row's cells, setting SGR where the style changes. Returns whether
/// the row wraps into the next one, in which case it is written in full and the
/// next row continues it without a line break.
fn write_row(out: &mut String, style: &mut Style, cells: Vec<&Cell>) -> bool {
    let wrapped = cells
        .last()
        .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE));
    let end = if wrapped {
        cells.len()
    } else {
        cells
            .iter()
            .rposition(|cell| !is_blank(cell))
            .map_or(0, |last| last + 1)
    };
    for cell in &cells[..end] {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let wanted = Style::of(cell);
        if wanted != *style {
            wanted.write(out);
            *style = wanted;
        }
        out.push(cell.c);
        if let Some(marks) = cell.zerowidth() {
            out.extend(marks);
        }
    }
    wrapped
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::grid::Grid;

    use super::*;

    fn model(cols: u16, rows: u16, bytes: &[u8]) -> TerminalModel {
        let mut model = TerminalModel::new(cols, rows);
        model.advance(bytes);
        model
    }

    /// The paint of `a`, played into a fresh model of the same size.
    fn repainted(a: &mut TerminalModel, cols: u16, rows: u16) -> TerminalModel {
        let mut b = TerminalModel::new(cols, rows);
        b.advance(&a.paint());
        b
    }

    fn same_cell(a: &Cell, b: &Cell) -> bool {
        a.c == b.c
            && a.fg == b.fg
            && a.bg == b.bg
            && a.flags == b.flags
            && a.zerowidth() == b.zerowidth()
            && a.underline_color() == b.underline_color()
    }

    fn assert_same(a: &TerminalModel, b: &TerminalModel) {
        let (ga, gb): (&Grid<Cell>, &Grid<Cell>) = (a.term().grid(), b.term().grid());
        assert_eq!(ga.history_size(), gb.history_size(), "history length");
        assert_eq!(ga.cursor.point, gb.cursor.point, "cursor");
        assert_eq!(a.term().mode(), b.term().mode(), "modes");
        assert_eq!(
            a.term().cursor_style(),
            b.term().cursor_style(),
            "cursor style"
        );
        let top = -(ga.history_size() as i32);
        for line in top..ga.screen_lines() as i32 {
            for column in 0..ga.columns() {
                let point = (Line(line), Column(column));
                let (ca, cb) = (&ga[point.0][point.1], &gb[point.0][point.1]);
                assert!(
                    same_cell(ca, cb),
                    "cell {line},{column} differs: {ca:?} vs {cb:?}"
                );
            }
        }
    }

    #[test]
    fn a_paint_rebuilds_history_screen_cursor_and_modes() {
        let mut bytes = Vec::new();
        for n in 0..30 {
            bytes.extend(format!("line {n}\r\n").bytes());
        }
        // A row that wraps over three rows, styles, wide and combining marks.
        bytes.extend(b"0123456789012345678901234567890123456789012345\r\n");
        bytes.extend(b"\x1b[1;31;44mbold red on blue\x1b[0m \x1b[38;5;208mindexed\x1b[0m ");
        bytes.extend(
            b"\x1b[38;2;1;2;3;48;2;9;8;7mtrue\x1b[0m \x1b[4munder\x1b[0m \x1b[7minv\x1b[0m\r\n",
        );
        bytes.extend("\u{4e16}x e\u{323}\u{302} caf\u{e9}\r\n".as_bytes());
        bytes.extend(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1h\x1b[5 q\x1b[?25l\x1b[3;5H");
        let mut a = model(20, 10, &bytes);
        assert!(
            a.term().grid().history_size() > 20,
            "the fixture has history"
        );
        let b = repainted(&mut a, 20, 10);
        assert_same(&a, &b);
    }

    #[test]
    fn a_paint_rebuilds_the_alternate_screen() {
        let mut a = model(
            20,
            6,
            b"primary\r\n\x1b[?1049h\x1b[2;3Halt \x1b[32mgreen\x1b[0m\x1b[?1000h",
        );
        let b = repainted(&mut a, 20, 6);
        assert!(b.term().mode().contains(TermMode::ALT_SCREEN));
        assert!(b.term().mode().contains(TermMode::MOUSE_REPORT_CLICK));
        assert_same(&a, &b);
    }

    #[test]
    fn a_sequence_split_across_chunks_is_one_sequence() {
        let mut a = TerminalModel::new(20, 4);
        a.advance(b"\x1b[3");
        a.advance(b"1mred\xe4\xb8");
        a.advance(b"\x96!");
        let b = model(20, 4, "\u{1b}[31mred\u{4e16}!".as_bytes());
        assert_same(&a, &b);
    }

    #[test]
    fn a_resize_changes_the_grid_the_paint_describes() {
        let mut a = model(20, 4, b"one\r\ntwo");
        a.resize(30, 6);
        let b = repainted(&mut a, 30, 6);
        assert_same(&a, &b);
    }

    #[test]
    fn a_paint_taken_mid_synchronized_update_shows_the_frame_so_far() {
        // BSU, half a frame, no ESU yet: the parser holds these bytes back.
        let mut a = model(20, 4, b"old\r\n\x1b[?2026h\x1b[Hnew frame");
        let b = repainted(&mut a, 20, 4);
        let text: String = (0..4)
            .map(|line| {
                (0..20)
                    .map(|column| b.term().grid()[Line(line)][Column(column)].c)
                    .collect::<String>()
            })
            .collect();
        assert!(text.contains("new frame"), "{text:?}");
    }
}

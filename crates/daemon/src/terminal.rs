//! The daemon's copy of a session's terminal: a `vt` terminal fed every
//! output chunk from the moment the process starts, so a GUI that attaches
//! later can be handed a first paint instead of the last megabyte of raw
//! bytes.

use vt::{Limits, Vt};

pub struct TerminalModel {
    vt: Vt,
}

impl TerminalModel {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            vt: Vt::new(
                cols,
                rows,
                Limits {
                    lines: protocol::SCROLLBACK_LINES,
                    bytes: protocol::SCROLLBACK_MAX_BYTES,
                },
            ),
        }
    }

    /// Parse output. The parser keeps its state between calls, so a sequence
    /// split across two chunks is still one sequence.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.vt.write(bytes);
        // The terminal's answers to the program's queries have no way back to
        // the PTY, so they are dropped rather than buffered forever.
        self.vt.take_replies();
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.vt.resize(cols, rows);
    }

    /// ANSI bytes that rebuild this terminal in a fresh parser of the same
    /// size: the retained history, the screen, the cursor and the modes.
    // ponytail: the engine's dump does not restore the primary screen under a
    // running alternate-screen program, nor the colour of a row that only has
    // a background; the program's next redraw sets what it needs.
    pub fn paint(&mut self) -> Vec<u8> {
        let mut out = b"\x1bc".to_vec();
        out.extend_from_slice(&self.vt.dump_vt());
        // The dump leaves a synchronized update (CSI ?2026h) open; end it so
        // the receiver draws the frame so far. The program's own end arrives
        // on the live stream.
        if self.vt.mode(2026) {
            out.extend_from_slice(b"\x1b[?2026l");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        lines: protocol::SCROLLBACK_LINES,
        bytes: protocol::SCROLLBACK_MAX_BYTES,
    };

    /// The DEC private modes whose state the paint has to hold again.
    const CHECKED_MODES: [u16; 12] = [1, 6, 7, 25, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 2004];

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

    /// A cell's text, with an unwritten cell read as the space it draws as.
    fn cell_text(cell: &vt::Cell) -> &str {
        if cell.text.is_empty() {
            " "
        } else {
            &cell.text
        }
    }

    /// An unwritten cell and one written as a space draw the same, and the
    /// old engine did not tell them apart either; everything else must match.
    fn same_cell(a: &vt::Cell, b: &vt::Cell) -> bool {
        cell_text(a) == cell_text(b)
            && a.fg == b.fg
            && a.bg == b.bg
            && a.bold == b.bold
            && a.italic == b.italic
            && a.faint == b.faint
            && a.underline == b.underline
            && a.underline_color == b.underline_color
            && a.strike == b.strike
            && a.wide == b.wide
    }

    fn assert_same(a: &mut Vt, b: &mut Vt) {
        let (fa, fb) = (a.frame(), b.frame());
        assert_eq!(fa.rows.len(), fb.rows.len(), "row count");
        for (line, (ra, rb)) in fa.rows.iter().zip(&fb.rows).enumerate() {
            for (x, (ca, cb)) in ra.iter().zip(rb.iter()).enumerate() {
                assert!(same_cell(ca, cb), "cell {line},{x}: {ca:?} vs {cb:?}");
            }
        }
        assert_eq!(fa.cursor_cell, fb.cursor_cell, "cursor");
        assert_eq!(a.history_size(), b.history_size(), "history length");
        assert_eq!(a.alt_screen(), b.alt_screen(), "alternate screen");
        assert_eq!(a.mouse_tracking(), b.mouse_tracking(), "mouse tracking");
        for mode in CHECKED_MODES {
            assert_eq!(a.mode(mode), b.mode(mode), "mode {mode}");
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
        assert!(a.vt.history_size() > 20, "the fixture has history");
        let mut b = repainted(&mut a, 20, 10);
        assert_same(&mut a.vt, &mut b.vt);
    }

    #[test]
    fn a_paint_rebuilds_the_alternate_screen() {
        let mut a = model(
            20,
            6,
            b"primary\r\n\x1b[?1049h\x1b[2;3Halt \x1b[32mgreen\x1b[0m\x1b[?1000h",
        );
        let mut b = repainted(&mut a, 20, 6);
        assert!(b.vt.alt_screen());
        assert!(b.vt.mouse_tracking());
        assert_same(&mut a.vt, &mut b.vt);
        // The primary screen under the program is not in the paint.
        b.advance(b"\x1b[?1049l");
        assert!(b.vt.frame().rows[0].iter().all(|cell| cell.text.is_empty()));
    }

    #[test]
    fn a_sequence_split_across_chunks_is_one_sequence() {
        let mut a = TerminalModel::new(20, 4);
        a.advance(b"\x1b[3");
        a.advance(b"1mred\xe4\xb8");
        a.advance(b"\x96!");
        let mut b = model(20, 4, "\u{1b}[31mred\u{4e16}!".as_bytes());
        assert_same(&mut a.vt, &mut b.vt);
    }

    #[test]
    fn a_resize_changes_the_grid_the_paint_describes() {
        let mut a = model(20, 4, b"one\r\ntwo");
        a.resize(30, 6);
        let mut b = repainted(&mut a, 30, 6);
        assert_same(&mut a.vt, &mut b.vt);
    }

    #[test]
    fn a_paint_taken_mid_synchronized_update_shows_the_frame_so_far() {
        // BSU, half a frame, no ESU yet: the terminal already holds the bytes.
        let mut a = model(20, 4, b"old\r\n\x1b[?2026h\x1b[Hnew frame");
        let mut b = repainted(&mut a, 20, 4);
        let frame = b.vt.frame();
        let text: String = frame
            .rows
            .iter()
            .flat_map(|row| row.iter().map(|cell| cell.text.as_str()))
            .collect();
        assert!(text.contains("new frame"), "{text:?}");
    }

    #[test]
    fn a_paint_is_a_valid_start_for_a_terminal_that_already_has_content() {
        let mut a = model(20, 4, b"one\r\ntwo\r\nthree");
        let mut b = Vt::new(20, 4, LIMITS);
        for i in 0..20 {
            b.write(format!("other {i}\r\n").as_bytes());
        }
        b.write(&a.paint());
        assert_same(&mut a.vt, &mut b);
    }
}

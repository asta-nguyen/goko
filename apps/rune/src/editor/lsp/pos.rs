//! UTF-16 LSP position conversion and the incremental-sync diff.
//!
//! Follows the vscode-languageserver-node model (see the Task 3 notes):
//! CRLF, lone CR and LF are all line breaks; a column counts UTF-16 code
//! units of the line's content *excluding* the line terminator; a position
//! can never sit between `\r` and `\n` of a CRLF pair or inside a UTF-16
//! surrogate pair; a range that contains a newline ends at the next line's
//! column 0. All offsets are byte offsets into the actual editor text —
//! never the on-disk original.

/// `(line, character)` for `byte`, 0-based, `character` in UTF-16 units.
/// `byte` is clamped to a char boundary and may not split a CRLF pair —
/// an offset between `\r` and `\n` resolves to the end of the previous
/// line, matching `offsetAt`'s clamp-before-EOL behaviour.
pub fn position_at(text: &str, byte: usize) -> (u32, u32) {
    let bytes = text.as_bytes();
    let mut byte = byte.min(bytes.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    if byte > 0 && byte < bytes.len() && bytes[byte - 1] == b'\r' && bytes[byte] == b'\n' {
        byte -= 1;
    }
    let (mut line, mut col, mut i) = (0u32, 0u32, 0usize);
    while i < byte {
        match bytes[i] {
            b'\r' => {
                let next = i + 1 + usize::from(bytes.get(i + 1) == Some(&b'\n'));
                if next <= byte {
                    line += 1;
                    col = 0;
                    i = next;
                } else {
                    break;
                }
            }
            b'\n' => {
                if i < byte {
                    line += 1;
                    col = 0;
                    i += 1;
                } else {
                    break;
                }
            }
            _ => {
                let ch = text[i..].chars().next().expect("char at i");
                col += ch.len_utf16() as u32;
                i += ch.len_utf8();
            }
        }
    }
    (line, col)
}

/// Byte offset of `(line, character)`; `character` counts UTF-16 units.
/// The line is clamped to the document, the column to the line's content
/// (positions cannot pass the line terminator), and a column landing
/// inside a surrogate pair resolves to the pair's start.
#[cfg(test)]
pub fn offset_at(text: &str, line: u32, character: u32) -> usize {
    let bytes = text.as_bytes();
    let (mut l, mut i) = (0u32, 0usize);
    while l < line && i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                i += 1 + usize::from(bytes.get(i + 1) == Some(&b'\n'));
                l += 1;
            }
            b'\n' => {
                i += 1;
                l += 1;
            }
            _ => i += text[i..].chars().next().expect("char at i").len_utf8(),
        }
    }
    if l < line {
        return bytes.len();
    }
    let mut col = 0u32;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => break,
            _ => {
                let ch = text[i..].chars().next().expect("char at i");
                let width = ch.len_utf16() as u32;
                if col + width > character {
                    break;
                }
                col += width;
                i += ch.len_utf8();
                if col == character {
                    break;
                }
            }
        }
    }
    i
}

/// UTF-16 length of `text` (what `rangeLength` carries).
pub fn utf16_len(text: &str) -> u32 {
    text.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Common prefix/suffix diff between the last-sent text and the current
/// text, as `(old_start, old_end, inserted)` byte ranges/`String` for one
/// `TextDocumentContentChangeEvent`. Char boundaries are preserved and a
/// boundary never splits a CRLF pair on either side; `None` when the
/// texts are identical (no event should be sent).
pub fn diff_change(old: &str, new: &str) -> Option<(usize, usize, String)> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = 0usize;
    while prefix < ob.len() && prefix < nb.len() && ob[prefix] == nb[prefix] {
        prefix += 1;
    }
    while prefix > 0 && !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    // A boundary between `\r` and `\n` (on either side) splits a CRLF:
    // pull the `\r` into the changed region.
    if prefix > 0
        && ob[prefix - 1] == b'\r'
        && (ob.get(prefix) == Some(&b'\n') || nb.get(prefix) == Some(&b'\n'))
    {
        prefix -= 1;
    }
    let mut suffix = 0usize;
    while suffix < ob.len() - prefix
        && suffix < nb.len() - prefix
        && ob[ob.len() - 1 - suffix] == nb[nb.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(ob.len() - suffix) || !new.is_char_boundary(nb.len() - suffix))
    {
        suffix -= 1;
    }
    // Same CRLF guard at the suffix boundary: the shared tail must not
    // begin between `\r` and `\n`.
    while suffix > 0 {
        let oe = ob.len() - suffix;
        let ne = nb.len() - suffix;
        let old_split = oe < ob.len() && ob[oe - 1] == b'\r' && ob[oe] == b'\n';
        let new_split = ne < nb.len() && nb[ne - 1] == b'\r' && nb[ne] == b'\n';
        if !old_split && !new_split {
            break;
        }
        suffix -= 1;
        while suffix > 0
            && (!old.is_char_boundary(ob.len() - suffix)
                || !new.is_char_boundary(nb.len() - suffix))
        {
            suffix -= 1;
        }
    }
    let old_end = ob.len() - suffix;
    let new_end = nb.len() - suffix;
    Some((prefix, old_end, new[prefix..new_end].to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_positions_round_trip() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        for (line, col) in [(0, 0), (0, 3), (1, 4), (1, 12), (2, 0), (2, 1), (3, 0)] {
            let byte = offset_at(text, line, col);
            assert_eq!(position_at(text, byte), (line, col), "{line}:{col}");
        }
        // Past the end clamps to the last position.
        assert_eq!(position_at(text, text.len() + 50), (3, 0));
        assert_eq!(offset_at(text, 99, 0), text.len());
    }

    #[test]
    fn utf16_columns_count_units_not_chars() {
        // 'é' is 1 UTF-16 unit, '😀' is 2 (a surrogate pair).
        let text = "aéb\n😀x\n";
        // bytes: a=0, é=1..3, b=3, \n=4, 😀=5..9, x=9, \n=10
        assert_eq!(position_at(text, 3), (0, 2)); // 'b' itself: a+é = 2 units before it
        assert_eq!(position_at(text, 4), (0, 3)); // '\n' → end of line 0
        assert_eq!(position_at(text, 5), (1, 0));
        assert_eq!(position_at(text, 9), (1, 2)); // after the surrogate pair
        assert_eq!(position_at(text, 10), (1, 3));
        // offset_at never splits the pair: col 1 inside 😀 → pair start.
        assert_eq!(offset_at(text, 1, 1), 5);
        assert_eq!(offset_at(text, 1, 2), 9);
        assert_eq!(offset_at(text, 1, 3), 10);
    }

    #[test]
    fn crlf_is_one_break_and_positions_cannot_split_it() {
        let text = "a\r\nb\rc\nd";
        // bytes: a0 \r1 \n2 b3 \r4 c5 \n6 d7
        assert_eq!(position_at(text, 3), (1, 0)); // 'b' after CRLF
        assert_eq!(position_at(text, 2), (0, 1)); // inside CRLF → end of line 0
        assert_eq!(position_at(text, 1), (0, 1)); // at \r → end of line 0
        assert_eq!(position_at(text, 5), (2, 0)); // 'c' after lone \r
        assert_eq!(position_at(text, 7), (3, 0)); // 'd' after \n
        assert_eq!(offset_at(text, 1, 0), 3);
        assert_eq!(offset_at(text, 2, 0), 5);
        assert_eq!(offset_at(text, 3, 0), 7);
        // A column past the line's content stops before the terminator.
        assert_eq!(offset_at(text, 0, 99), 1);
        assert_eq!(offset_at(text, 1, 99), 4);
    }

    #[test]
    fn combining_marks_count_units() {
        // 'e' + U+0301 combining acute = 2 UTF-16 units, 1 grapheme.
        let text = "e\u{301}x";
        assert_eq!(position_at(text, 3), (0, 2)); // after e+́
        assert_eq!(offset_at(text, 0, 2), 3);
        assert_eq!(offset_at(text, 0, 1), 1); // inside the cluster floors to 'e' end? col1 is mid-cluster
    }

    #[test]
    fn diff_inserts_middle() {
        let (s, e, t) = diff_change("fn main() {}", "fn main() {\n    x();\n}").unwrap();
        assert_eq!((&"fn main() {}"[s..e], t.as_str()), ("", "\n    x();\n"));
    }

    #[test]
    fn diff_deletes_and_replaces() {
        let (s, e, t) = diff_change("hello world", "hello there world").unwrap();
        assert_eq!((&"hello world"[s..e], t.as_str()), ("", "there "));
        let (s, e, t) = diff_change("abc", "axc").unwrap();
        assert_eq!((&"abc"[s..e], t.as_str()), ("b", "x"));
        assert!(diff_change("same", "same").is_none());
    }

    #[test]
    fn diff_never_splits_crlf_or_surrogates() {
        // CRLF removed: the \r must join the changed region, not leave a
        // dangling \n or a mid-CRLF boundary.
        let old = "a\r\nb";
        let new = "a\nb";
        let (s, e, t) = diff_change(old, new).unwrap();
        let (ls, lc) = position_at(old, s);
        let (le, _) = position_at(old, e);
        // Range must not start/end inside the CRLF.
        assert!(!(old.as_bytes()[s - 1] == b'\r' && old.as_bytes()[s] == b'\n'));
        assert!(!(old.as_bytes()[e - 1] == b'\r' && old.as_bytes()[e] == b'\n'));
        assert_eq!((&old[s..e], t.as_str()), ("\r\n", "\n"));
        let _ = (ls, lc, le);
        // Surrogate pair at the edit boundary stays whole.
        let (s, e, t) = diff_change("a😀b", "a😀xb").unwrap();
        assert!("a😀b".is_char_boundary(s) && "a😀b".is_char_boundary(e));
        assert_eq!((&"a😀b"[s..e], t.as_str()), ("", "x"));
        // Emoji prefix split attempt: shared lead bytes must not cut the char.
        let (s, _e, t) = diff_change("€uro", "‚uro").unwrap();
        assert!("€uro".is_char_boundary(s));
        assert_eq!(t, "‚");
    }
}

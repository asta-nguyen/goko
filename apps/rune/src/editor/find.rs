//! In-file find for the native code surface: query state, Unicode-correct
//! byte-range matches, previous/next navigation and the match-count label.
//! Pure data plus a [`TextEdit`] for the query field — no layout or I/O.

use rune::TextEdit;
use std::ops::Range;

/// Find-panel state owned by [`crate::editor::code::CodeState`]. The query is
/// a real single-line [`TextEdit`] so IME and selection behave natively.
#[derive(Debug)]
pub struct FindState {
    /// Whether the panel is showing.
    pub open: bool,
    /// The query draft.
    pub query: TextEdit,
    /// Byte ranges of every match in the document, document order. Complete
    /// — the 1 MiB file limit bounds the count; rendering stays capped in the
    /// view, navigation is not.
    pub matches: Vec<Range<usize>>,
    /// Index into `matches` of the current match; `None` when there are none.
    pub active: Option<usize>,
    /// Case-sensitive matching (the panel toggles it).
    pub case_sensitive: bool,
}

impl Default for FindState {
    fn default() -> Self {
        Self::new()
    }
}

impl FindState {
    pub fn new() -> FindState {
        FindState {
            open: false,
            query: TextEdit::new(""),
            matches: Vec::new(),
            active: None,
            case_sensitive: false,
        }
    }

    /// Show the panel, keeping the last query and matches for reopening.
    pub fn open_panel(&mut self) {
        self.open = true;
    }

    /// Hide the panel; query, matches and the active match survive a reopen.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Replace the query text (seeding from a selection, say) and recompute.
    pub fn set_query(&mut self, text: &str) {
        self.query.replace_text(text);
        self.query.set_caret(self.query.text.len());
    }

    /// Recompute `matches` against `text`, keeping `active` on the same range
    /// when it still exists, else moving to the first match.
    pub fn refresh(&mut self, text: &str) {
        let previous = self.active.and_then(|i| self.matches.get(i).cloned());
        self.matches = search_matches(text, &self.query.text, self.case_sensitive);
        self.active = if self.matches.is_empty() {
            None
        } else {
            Some(
                previous
                    .and_then(|r| self.matches.iter().position(|m| *m == r))
                    .unwrap_or(0),
            )
        };
    }

    /// The active match's byte range, if any.
    pub fn active_range(&self) -> Option<Range<usize>> {
        self.active.and_then(|i| self.matches.get(i).cloned())
    }

    /// Advance to the next match, wrapping; `None` when there are none.
    pub fn next(&mut self) -> Option<Range<usize>> {
        self.step(1)
    }

    /// Go to the previous match, wrapping; `None` when there are none.
    pub fn prev(&mut self) -> Option<Range<usize>> {
        self.step(-1)
    }

    fn step(&mut self, delta: isize) -> Option<Range<usize>> {
        if self.matches.is_empty() {
            self.active = None;
            return None;
        }
        let len = self.matches.len() as isize;
        let current = self
            .active
            .map(|i| i as isize)
            .unwrap_or(if delta > 0 { -1 } else { 0 });
        let next = (current + delta).rem_euclid(len) as usize;
        self.active = Some(next);
        self.matches.get(next).cloned()
    }

    /// `"3 of 42"`, `"42 matches"`, `"No results"` or `""` for an empty query.
    pub fn count_label(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        let n = self.matches.len();
        if n == 0 {
            return "No results".to_string();
        }
        match self.active {
            Some(i) => format!("{} of {n}", i + 1),
            None => format!("{n} matches"),
        }
    }
}

/// All non-overlapping matches of `query` in `text` as UTF-8 byte ranges,
/// always on character boundaries. Empty `query` matches nothing.
/// Case-sensitive convenience for tests — production refreshes through
/// [`FindState::refresh`].
#[cfg(test)]
pub fn search(text: &str, query: &str) -> Vec<Range<usize>> {
    search_matches(text, query, true)
}

/// Case-insensitive by default when `case_sensitive` is false. Complete
/// results — the document's 1 MiB cap bounds the count.
fn search_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    if case_sensitive {
        for (start, m) in text.match_indices(query) {
            out.push(start..start + m.len());
        }
        return out;
    }
    let mut pos = 0;
    while pos <= text.len() {
        // `pos` is always a char boundary: it starts at 0 and advances by whole matches.
        if let Some(end) = ci_match_end(text, pos, query) {
            out.push(pos..end);
            pos = end;
        } else {
            pos += text[pos..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    out
}

/// Byte end of `query` matched case-insensitively at `pos` (a char boundary),
/// or `None`. Comparison is per `char` on lowercase iterators, so multi-char
/// folds ('İ' → "i̇") simply don't match a single char.
fn ci_match_end(text: &str, pos: usize, query: &str) -> Option<usize> {
    let mut t = text[pos..].chars();
    let mut end = pos;
    for qc in query.chars() {
        let tc = t.next()?;
        if !tc.to_lowercase().eq(qc.to_lowercase()) {
            return None;
        }
        end += tc.len_utf8();
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_byte_ranges() {
        let text = "fn main() { println!(\"hi\"); }";
        assert_eq!(search(text, "println"), vec![12..19]);
        assert_eq!(search(text, "n"), vec![1..2, 6..7, 15..16, 18..19]);
        assert!(search(text, "").is_empty());
        assert!(search(text, "missing").is_empty());
    }

    #[test]
    fn search_is_unicode_correct() {
        // 'é' is two bytes; matches must start and end on char boundaries.
        let text = "café naïve café";
        let ranges = search(text, "café");
        assert_eq!(ranges.len(), 2);
        for r in &ranges {
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
            assert_eq!(&text[r.clone()], "café");
        }
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[1].start, text.rfind("café").unwrap());
        // Multibyte query against multibyte text.
        let text = "のテキストと";
        let ranges = search(text, "テキ");
        assert_eq!(ranges, vec![3..9]);
        assert!(
            ranges
                .iter()
                .all(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        );
        // A query ending mid-grapheme still lands on scalar boundaries.
        let ranges = search("áb", "b");
        assert_eq!(ranges, vec![3..4]);
    }

    #[test]
    fn search_case_insensitive_and_sensitive() {
        let text = "Foo foo FOO bar";
        assert_eq!(search_matches(text, "foo", false).len(), 3);
        assert_eq!(search_matches(text, "foo", true).len(), 1);
        assert_eq!(search_matches("Émile éMILE", "émile", false).len(), 2);
        // Per-char `eq_ignore_case`: 'ß' does not fold to "ss".
        assert_eq!(search_matches("Straße STRASSE", "strasse", false).len(), 1);
    }

    #[test]
    fn search_does_not_overlap() {
        assert_eq!(search("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(search("aaa", "aa"), vec![0..2]);
    }

    #[test]
    fn find_state_navigation_wraps() {
        let mut f = FindState::new();
        f.set_query("a");
        f.refresh("a b a c a");
        assert_eq!(f.matches, vec![0..1, 4..5, 8..9]);
        assert_eq!(f.active, Some(0));
        assert_eq!(f.active_range(), Some(0..1));
        assert_eq!(f.next(), Some(4..5));
        assert_eq!(f.next(), Some(8..9));
        assert_eq!(f.next(), Some(0..1), "wraps to first");
        assert_eq!(f.prev(), Some(8..9), "wraps to last");
        assert_eq!(f.count_label(), "3 of 3");
    }

    #[test]
    fn find_state_refresh_preserves_active_range() {
        let mut f = FindState::new();
        f.set_query("ab");
        f.refresh("ab x ab y ab");
        f.next(); // active = second 'ab' at 5..7
        assert_eq!(f.active_range(), Some(5..7));
        f.refresh("ab x ab y ab z");
        assert_eq!(f.active_range(), Some(5..7), "same range keeps its index");
        f.refresh("ab x ab"); // third match gone
        assert_eq!(f.active, Some(1), "falls back to first remaining");
        f.refresh("nothing here");
        assert_eq!(f.active, None);
        assert_eq!(f.count_label(), "No results");
    }

    #[test]
    fn find_state_empty_query_and_close() {
        let mut f = FindState::new();
        f.open_panel();
        assert!(f.open);
        f.set_query("");
        f.refresh("text");
        assert!(f.matches.is_empty());
        assert_eq!(f.count_label(), "");
        f.close();
        assert!(!f.open);
        assert!(f.query.text.is_empty());
    }

    #[test]
    fn find_state_steps_on_empty_matches() {
        let mut f = FindState::new();
        f.set_query("zzz");
        f.refresh("abc");
        assert_eq!(f.next(), None);
        assert_eq!(f.prev(), None);
        assert_eq!(f.active, None);
    }

    #[test]
    fn find_state_stores_and_navigates_all_matches() {
        // No hidden result cutoff: every match in the (1 MiB-bounded)
        // document is stored and reachable by next/prev.
        let text = "hit\n".repeat(5000);
        let mut f = FindState::new();
        f.set_query("hit");
        f.refresh(&text);
        assert_eq!(f.matches.len(), 5000, "all matches stored, no cutoff");
        assert_eq!(f.count_label(), "1 of 5000");
        for _ in 0..4999 {
            assert!(f.next().is_some());
        }
        assert_eq!(f.active, Some(4999));
        assert_eq!(f.active_range(), Some(19996..19999));
        assert_eq!(f.count_label(), "5000 of 5000");
        assert_eq!(f.prev(), Some(19992..19995), "steps back off the end");
    }
}

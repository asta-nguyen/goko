//! Git blame for the open document: who last touched each line.
//!
//! The blame read is a per-file Git query, so it lives on the same worker
//! lane as every other checkout read ([`crate::editor::backend::Req::Blame`]).
//! The result is stored on the open document's [`CodeState`] and drawn as a
//! column beside the code, one annotation per line.
//!
//! A dirty buffer breaks the mapping: Git blames the file *on disk*, so a
//! line added in the buffer would shift every later line. The column is
//! therefore hidden while the draft is dirty and re-read after the save.

use app_core::git_view::GitViewBlame;

use crate::editor::backend::Req;
use crate::editor::state::EditorState;

/// Settings key for the inline-blame toggle (`1` on, `0` off).
pub const BLAME_KEY: &str = "goko.editor.blame";

/// One line's blame: the author who last touched it and when.
#[derive(Clone, Debug, Default)]
pub struct BlameLine {
    /// Author of the commit that last touched the line.
    pub author: String,
    /// Commit time, epoch milliseconds.
    pub time: u64,
    /// The line was changed in the working tree but never committed.
    pub uncommitted: bool,
}

/// The blame of one file: one entry per line, in file order.
#[derive(Clone, Debug, Default)]
pub struct Blame {
    /// Per-line entries, index 0 is the file's first line.
    pub lines: Vec<BlameLine>,
}

impl Blame {
    /// The entry for a 1-based line number, if the file has one.
    pub fn line(&self, number: u64) -> Option<&BlameLine> {
        number
            .checked_sub(1)
            .and_then(|i| self.lines.get(i as usize))
    }
}

/// The current wall clock, in milliseconds since the epoch.
fn now_ms() -> u64 {
    crate::data::now_ms().max(0) as u64
}

/// Convert the worker's blame into the per-line entries the surface draws.
/// The file's line count comes from the highest blamed line — a commit may
/// have skipped lines (a binary chunk, a merge), so the entries are sparse.
pub fn from_view(view: GitViewBlame) -> Blame {
    let count = view.lines.iter().map(|entry| entry.line).max().unwrap_or(0);
    let mut lines = vec![BlameLine::default(); count as usize];
    for entry in view.lines {
        let Some(slot) = entry
            .line
            .checked_sub(1)
            .and_then(|i| lines.get_mut(i as usize))
        else {
            continue;
        };
        slot.author = entry.author;
        slot.time = entry.time;
        slot.uncommitted = entry.uncommitted;
    }
    Blame { lines }
}

/// The compact age of a commit: minutes, hours, days, months, years.
pub fn age(epoch_ms: u64, now_ms: u64) -> String {
    const MINUTE: u64 = 60_000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    let e = now_ms.saturating_sub(epoch_ms);
    if e < MINUTE {
        return "now".to_string();
    }
    if e < HOUR {
        return format!("{}m", e / MINUTE);
    }
    if e < DAY {
        return format!("{}h", e / HOUR);
    }
    if e < MONTH {
        return format!("{}d", e / DAY);
    }
    if e < YEAR {
        return format!("{}mo", e / MONTH);
    }
    format!("{}y", e / YEAR)
}

/// The annotation drawn for a line: its author and how long ago the commit
/// landed. An uncommitted line says so instead of naming an author.
pub fn label(line: &BlameLine, now_ms: u64) -> String {
    if line.uncommitted {
        return "uncommitted".to_string();
    }
    if line.author.is_empty() {
        return "-".to_string();
    }
    format!("{} · {}", line.author, age(line.time, now_ms))
}

/// The rendered column: one annotation per line of the document, in line
/// order, so each row lines up with its code line.
pub fn render(blame: &Blame, lines: usize, now_ms: u64) -> String {
    let mut out = String::new();
    for number in 1..=lines {
        if number > 1 {
            out.push('\n');
        }
        if let Some(line) = blame.line(number as u64) {
            out.push_str(&label(line, now_ms));
        } else {
            // A line Git did not attribute (a skipped chunk) still needs a
            // row, or the annotations below it would drift up a line.
            out.push('-');
        }
    }
    out
}

/// Ask the worker for the open document's blame.
///
/// Returns `false` when the request could not be queued (no checkout, no
/// project, or a full queue) — the caller retries later. A dirty buffer or
/// a disabled column asks for nothing: neither can be annotated.
pub fn request(ed: &mut EditorState) -> bool {
    if !ed.blame_enabled {
        return false;
    }
    let Some(doc) = ed.doc.as_ref() else {
        return false;
    };
    // The annotations describe the file on disk; a dirty buffer's lines no
    // longer match it.
    if doc.dirty() {
        return false;
    }
    let Some(checkout) = ed.checkout.clone() else {
        return false;
    };
    let Some(key) = ed.checkout_key.clone() else {
        return false;
    };
    let path = doc.path.clone();
    ed.enqueue_read(checkout, key, Req::Blame { path })
}

/// Apply a blame reply. The result belongs to the document that was open
/// when it was asked for, so a reply for a since-replaced file is dropped.
pub fn apply(ed: &mut EditorState, path: &str, view: GitViewBlame) {
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    if doc.path != path {
        return;
    }
    let blame = from_view(view);
    let lines = doc.code.line_count();
    let now = now_ms();
    doc.code.blame_text = render(&blame, lines, now);
    doc.code.blame_bucket = now / 60_000;
    doc.code.blame_error = None;
    doc.code.blame = Some(blame);
}

/// A failed blame read leaves the previous annotations in place — they still
/// describe the last saved content — but the column's error line explains why
/// nothing arrived.
pub fn apply_error(ed: &mut EditorState, path: &str, error: &str) {
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    if doc.path == path {
        doc.code.blame_error = Some(error.to_string());
    }
}

/// Refresh the rendered column: the relative age of a commit rolls over
/// while the document stays open, so the text is rebuilt once a minute.
/// Returns `true` when the rendered text changed.
pub fn poll(ed: &mut EditorState) -> bool {
    let now = now_ms();
    let bucket = now / 60_000;
    let Some(doc) = ed.doc.as_mut() else {
        return false;
    };
    if doc.code.blame_bucket == bucket {
        return false;
    }
    doc.code.blame_bucket = bucket;
    let Some(blame) = doc.code.blame.clone() else {
        return false;
    };
    let lines = doc.code.line_count();
    let text = render(&blame, lines, now);
    if text == doc.code.blame_text {
        return false;
    }
    doc.code.blame_text = text;
    true
}

/// Mirror the toggle and the draft's state into the column's visibility: the
/// annotations describe the file on disk, so a dirty buffer hides them.
pub fn sync_visibility(ed: &mut EditorState) {
    let enabled = ed.blame_enabled;
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    doc.code.blame_visible = enabled && !doc.dirty();
}

/// Turn the inline-blame column on or off and persist the choice.
pub fn set_enabled(ed: &mut EditorState, enabled: bool) {
    ed.set_blame_enabled(enabled);
    sync_visibility(ed);
    if enabled {
        // The column was empty: ask for the annotations it will show.
        request(ed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::state::checkout_key;
    use app_core::git_view::GitViewBlameLine;
    use app_core::git_view::GitViewFile;
    use std::path::PathBuf;

    fn file(text: &str) -> GitViewFile {
        GitViewFile {
            path: "a.rs".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    fn editor() -> EditorState {
        EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle())
    }

    fn state_with_doc() -> EditorState {
        let mut e = editor();
        e.project_id = Some("p1".into());
        e.checkout = Some(PathBuf::from("/tmp/co"));
        e.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        e.doc = Some(OpenDoc::new("a.rs", &file("fn main() {}")));
        e
    }

    fn view(entries: &[(u64, &str, u64)]) -> GitViewBlame {
        GitViewBlame {
            path: "a.rs".into(),
            lines: entries
                .iter()
                .map(|(line, author, time)| GitViewBlameLine {
                    line: *line,
                    author: (*author).into(),
                    time: *time,
                    commit: "abc".into(),
                    uncommitted: false,
                })
                .collect(),
        }
    }

    #[test]
    fn entries_land_on_their_line_numbers() {
        let blame = from_view(view(&[(1, "Alice", 10), (3, "Bob", 20)]));
        assert_eq!(blame.lines.len(), 3);
        assert_eq!(blame.line(1).unwrap().author, "Alice");
        assert_eq!(blame.line(2).unwrap().author, "");
        assert_eq!(blame.line(3).unwrap().author, "Bob");
        assert!(blame.line(4).is_none());
    }

    #[test]
    fn a_reply_for_another_document_is_dropped() {
        let mut e = state_with_doc();
        apply(&mut e, "b.rs", view(&[(1, "Alice", 10)]));
        assert!(e.doc.as_ref().unwrap().code.blame.is_none());
        apply(&mut e, "a.rs", view(&[(1, "Alice", 10)]));
        assert!(e.doc.as_ref().unwrap().code.blame.is_some());
    }

    #[test]
    fn a_dirty_buffer_hides_the_column() {
        let mut e = state_with_doc();
        apply(&mut e, "a.rs", view(&[(1, "Alice", 10)]));
        sync_visibility(&mut e);
        assert!(e.doc.as_ref().unwrap().code.blame_visible);
        e.doc.as_mut().unwrap().code.edit.insert("x");
        sync_visibility(&mut e);
        assert!(!e.doc.as_ref().unwrap().code.blame_visible);
    }

    #[test]
    fn the_toggle_hides_the_column() {
        let mut e = state_with_doc();
        apply(&mut e, "a.rs", view(&[(1, "Alice", 10)]));
        sync_visibility(&mut e);
        assert!(e.doc.as_ref().unwrap().code.blame_visible);
        set_enabled(&mut e, false);
        assert!(!e.doc.as_ref().unwrap().code.blame_visible);
        assert!(!e.doc.as_ref().unwrap().code.blame_enabled);
    }

    #[test]
    fn a_dirty_buffer_is_never_blamed() {
        let mut e = state_with_doc();
        e.doc.as_mut().unwrap().code.edit.insert("x");
        assert!(!request(&mut e));
    }

    #[test]
    fn the_column_renders_one_row_per_line() {
        let blame = from_view(view(&[(1, "Alice", 0)]));
        let text = render(&blame, 3, 60_000);
        let rows: Vec<&str> = text.split('\n').collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], "Alice · 1m");
        assert_eq!(rows[1], "-");
        assert_eq!(rows[2], "-");
    }

    #[test]
    fn the_label_names_the_author_and_the_age() {
        let line = BlameLine {
            author: "Alice".into(),
            time: 0,
            uncommitted: false,
        };
        assert_eq!(label(&line, 60_000), "Alice · 1m");
        assert_eq!(label(&line, 3 * 24 * 3_600_000), "Alice · 3d");
        let uncommitted = BlameLine {
            author: "Not Committed Yet".into(),
            time: 0,
            uncommitted: true,
        };
        assert_eq!(label(&uncommitted, 0), "uncommitted");
    }
}

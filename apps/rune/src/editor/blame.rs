//! Git blame for the open document: who last touched each line.
//!
//! The blame read is a per-file Git query, so it lives on the same worker
//! lane as every other checkout read ([`crate::editor::backend::Req::Blame`]).
//! The result is stored on the open document's [`CodeState`] and drawn inline
//! at the end of the caret's line.
//!
//! A dirty buffer breaks the mapping: Git blames the file *on disk*, so a
//! line added in the buffer would shift every later line. The annotation is
//! therefore hidden while the draft is dirty and re-read after the save.

use app_core::git_view::GitViewBlame;
use rune::Tx;

use crate::State;
use crate::editor::backend::Req;
use crate::editor::navigation::{PendingNav, navigate};
use crate::editor::state::{EditorState, Section};

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
    /// Full oid of the commit that last touched the line.
    pub commit: String,
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
pub fn now_ms() -> u64 {
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
        slot.commit = entry.commit;
    }
    Blame { lines }
}

/// The commit a press on a line's annotation opens: committed lines only.
pub fn commit_of(blame: &Blame, number: u64) -> Option<&str> {
    blame
        .line(number)
        .filter(|line| !line.uncommitted && !line.commit.is_empty())
        .map(|line| line.commit.as_str())
}

/// Show `commit` in the History graph. `jump_to` is only set once History is
/// really the active section (a draft guard may have held the switch back), or
/// it would fire on some later visit.
pub fn open_commit(s: &mut State, commit: &str, tx: &mut Tx) {
    navigate(s, PendingNav::SetSection(Section::History), tx);
    if s.editor.section == Section::History {
        s.editor.history.jump_to = Some(commit.to_string());
    }
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

/// The annotation for a 1-based line number, when Git attributed it. A line
/// in a skipped chunk (or past the file's blamed lines) has no author and
/// nothing to draw.
pub fn line_label(blame: &Blame, number: u64, now_ms: u64) -> Option<String> {
    let line = blame.line(number)?;
    if line.author.is_empty() && !line.uncommitted {
        return None;
    }
    Some(label(line, now_ms))
}

/// Ask the worker for the open document's blame.
///
/// Returns `false` when the request could not be queued (no checkout, no
/// project, or a full queue) — the caller retries later. A dirty buffer or
/// disabled blame asks for nothing: neither can be annotated.
pub fn request(ed: &mut EditorState) -> bool {
    if !ed.blame_enabled || ed.folder_project {
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
    doc.code.blame_bucket = now_ms() / 60_000;
    doc.code.blame_error = None;
    doc.code.blame = Some(from_view(view));
}

/// A failed blame read leaves the previous annotations in place — they still
/// describe the last saved content — but the error bar explains why nothing
/// arrived.
pub fn apply_error(ed: &mut EditorState, path: &str, error: &str) {
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    if doc.path == path {
        doc.code.blame_error = Some(error.to_string());
    }
}

/// The relative age of a commit rolls over while the document stays open, so
/// the caret line is redrawn once a minute. Returns `true` when that redraw is
/// due and the annotation is showing.
pub fn poll(ed: &mut EditorState) -> bool {
    let bucket = now_ms() / 60_000;
    let Some(doc) = ed.doc.as_mut() else {
        return false;
    };
    if doc.code.blame_bucket == bucket {
        return false;
    }
    doc.code.blame_bucket = bucket;
    doc.code.blame_visible && doc.code.blame.is_some()
}

/// Mirror the toggle and the draft's state into whether the annotation is
/// drawn: it describes the file on disk, so a dirty buffer hides it.
pub fn sync_visibility(ed: &mut EditorState) {
    let enabled = ed.blame_enabled;
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    doc.code.blame_visible = enabled && !doc.dirty();
}

/// Turn inline blame on or off and persist the choice.
pub fn set_enabled(ed: &mut EditorState, enabled: bool) {
    ed.set_blame_enabled(enabled);
    sync_visibility(ed);
    if enabled {
        // Nothing was read while it was off: ask for the annotation it will show.
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
    fn a_dirty_buffer_hides_the_annotation() {
        let mut e = state_with_doc();
        apply(&mut e, "a.rs", view(&[(1, "Alice", 10)]));
        sync_visibility(&mut e);
        assert!(e.doc.as_ref().unwrap().code.blame_visible);
        e.doc.as_mut().unwrap().code.edit.insert("x");
        sync_visibility(&mut e);
        assert!(!e.doc.as_ref().unwrap().code.blame_visible);
    }

    #[test]
    fn the_toggle_hides_the_annotation() {
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
    fn a_line_with_no_blame_entry_has_no_label() {
        let blame = from_view(view(&[(1, "Alice", 0)]));
        assert_eq!(line_label(&blame, 1, 60_000).as_deref(), Some("Alice · 1m"));
        // A skipped chunk and a line past the end have nothing to draw.
        assert_eq!(line_label(&from_view(view(&[(2, "Bob", 0)])), 1, 0), None);
        assert_eq!(line_label(&blame, 9, 60_000), None);
    }

    #[test]
    fn the_age_redraw_is_due_once_a_minute_and_only_while_showing() {
        let mut e = state_with_doc();
        apply(&mut e, "a.rs", view(&[(1, "Alice", 10)]));
        sync_visibility(&mut e);
        assert!(!poll(&mut e), "same minute as the read");
        e.doc.as_mut().unwrap().code.blame_bucket -= 1;
        assert!(poll(&mut e), "a minute later the age is redrawn");
        e.doc.as_mut().unwrap().code.blame_bucket -= 1;
        e.doc.as_mut().unwrap().code.edit.insert("x");
        sync_visibility(&mut e);
        assert!(!poll(&mut e), "a blank annotation needs no redraw");
    }

    #[test]
    fn the_label_names_the_author_and_the_age() {
        let line = BlameLine {
            author: "Alice".into(),
            ..Default::default()
        };
        assert_eq!(label(&line, 60_000), "Alice · 1m");
        assert_eq!(label(&line, 3 * 24 * 3_600_000), "Alice · 3d");
        let uncommitted = BlameLine {
            author: "Not Committed Yet".into(),
            uncommitted: true,
            ..Default::default()
        };
        assert_eq!(label(&uncommitted, 0), "uncommitted");
    }

    #[test]
    fn only_a_committed_line_opens_its_commit() {
        let mut v = view(&[(1, "Alice", 10), (2, "Bob", 20), (3, "", 0)]);
        v.lines[1].uncommitted = true;
        v.lines[2].commit.clear();
        let blame = from_view(v);
        assert_eq!(commit_of(&blame, 1), Some("abc"));
        assert_eq!(commit_of(&blame, 2), None, "uncommitted has no commit");
        assert_eq!(commit_of(&blame, 3), None, "no oid to open");
        assert_eq!(commit_of(&blame, 9), None, "past the end");
    }

    #[test]
    fn opening_a_commit_switches_to_history_and_queues_the_jump() {
        let mut s = State::new(crate::test_handle());
        s.editor.section = Section::Files;
        let mut tx = Tx::default();
        open_commit(&mut s, "abc", &mut tx);
        assert_eq!(s.editor.section, Section::History);
        assert_eq!(s.editor.history.jump_to.as_deref(), Some("abc"));
    }
}

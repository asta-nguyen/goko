//! The change gutter: which lines of the open file the working tree changed
//! against `HEAD`, drawn as thin bars beside the line numbers.
//!
//! Like blame, this is a per-file Git read on the worker lane
//! ([`Req::LineChanges`]) and it describes the file *on disk*, so the bars are
//! hidden while the draft is dirty and re-read after the save.

use app_core::git_view::GitViewLineChange;

use crate::editor::backend::Req;
use crate::editor::state::EditorState;

/// Ask the worker for the open document's changed lines.
///
/// Returns `false` when nothing was queued: no document, a dirty buffer (its
/// lines no longer match the disk), or no checkout.
pub fn request(ed: &mut EditorState) -> bool {
    let Some(doc) = ed.doc.as_ref() else {
        return false;
    };
    if doc.dirty() {
        return false;
    }
    let (Some(checkout), Some(key)) = (ed.checkout.clone(), ed.checkout_key.clone()) else {
        return false;
    };
    let path = doc.path.clone();
    ed.enqueue_read(checkout, key, Req::LineChanges { path })
}

/// Apply a reply. It belongs to the document it was asked for, so a reply for
/// a since-replaced file is dropped.
pub fn apply(ed: &mut EditorState, path: &str, changes: Vec<GitViewLineChange>) {
    if let Some(doc) = ed.doc.as_mut().filter(|d| d.path == path) {
        doc.code.line_changes = changes;
    }
}

/// A failed read (no `HEAD` yet, a path Git rejects) just leaves no bars.
pub fn apply_error(ed: &mut EditorState, path: &str) {
    apply(ed, path, Vec::new());
}

/// The bars describe the file on disk, so a dirty buffer hides them.
pub fn sync_visibility(ed: &mut EditorState) {
    if let Some(doc) = ed.doc.as_mut() {
        doc.code.changes_visible = !doc.dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::state::checkout_key;
    use app_core::git_view::GitViewFile;
    use std::path::PathBuf;

    fn state_with_doc() -> EditorState {
        let mut e = EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle());
        e.project_id = Some("p1".into());
        e.checkout = Some(PathBuf::from("/tmp/co"));
        e.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        let file = GitViewFile {
            path: "a.rs".into(),
            kind: "text",
            text: Some("fn main() {}".into()),
            truncated: false,
            size: 12,
        };
        e.doc = Some(OpenDoc::new("a.rs", &file));
        e
    }

    fn span() -> Vec<GitViewLineChange> {
        vec![GitViewLineChange {
            start: 1,
            count: 1,
            kind: "added",
        }]
    }

    #[test]
    fn a_reply_for_another_document_is_dropped() {
        let mut e = state_with_doc();
        apply(&mut e, "b.rs", span());
        assert!(e.doc.as_ref().unwrap().code.line_changes.is_empty());
        apply(&mut e, "a.rs", span());
        assert_eq!(e.doc.as_ref().unwrap().code.line_changes.len(), 1);
        apply_error(&mut e, "a.rs");
        assert!(e.doc.as_ref().unwrap().code.line_changes.is_empty());
    }

    #[test]
    fn a_dirty_buffer_hides_the_bars_and_is_never_read() {
        let mut e = state_with_doc();
        sync_visibility(&mut e);
        assert!(e.doc.as_ref().unwrap().code.changes_visible);
        e.doc.as_mut().unwrap().code.edit.insert("x");
        sync_visibility(&mut e);
        assert!(!e.doc.as_ref().unwrap().code.changes_visible);
        assert!(!request(&mut e));
    }
}

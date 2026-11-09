//! Syntax-highlight pipeline for the open document.
//!
//! `CodeState` owns the `TextEdit` and applies [`HighlightResult`]s only
//! while `result.revision == code.revision`; this module owns everything
//! between an edit and that result:
//!
//! - [`mark_edit`] records the edit instant (input callback path; programmatic
//!   edits like `OpenDoc::discard` set `highlight_dirty_at` themselves).
//! - [`poll`] (from `AppSpec::poll`) debounces ~120 ms, then enqueues ONE
//!   `Req::Highlight` carrying path + revision + current text. A job
//!   already in flight suppresses new ones; edits during flight re-arm
//!   `highlight_dirty_at` so the reply's wake re-issues the newer revision.
//! - [`apply`] stores the worker's result only when path AND revision still
//!   match the live doc — stale replies are dropped, never rendered.
//!
//! Jobs ride the shared backend lane: they are pure CPU
//! (`syntax::highlight_revision` is `Send`-safe, owned ranges only), need
//! no Git or storage access, and are generation-tagged like every other
//! read so a superseded reply dies at the gate before reaching `apply`.

use std::time::{Duration, Instant};

use rune::Tx;

use crate::editor::backend::Req;
use crate::editor::state::EditorState;
use crate::editor::syntax::HighlightResult;

/// Quiet window after the last edit before a highlight job is issued —
/// long enough to coalesce a typing burst, short enough to feel instant.
const DEBOUNCE: Duration = Duration::from_millis(120);

/// Record a text mutation on the open doc (the `on_changed` input path).
pub fn mark_edit(ed: &mut EditorState) {
    if let Some(doc) = ed.doc.as_mut() {
        doc.highlight_dirty_at = Some(Instant::now());
    }
}

/// `AppSpec::poll` hook. Enqueues the debounced highlight for the open doc;
/// re-arms `wake_after` while inside the debounce window so the next poll
/// lands after the quiet period. Returns `false` — queueing a highlight
/// changes no visible state.
pub fn poll(ed: &mut EditorState, tx: &mut Tx) {
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    let Some(dirty_at) = doc.highlight_dirty_at else {
        return;
    };
    let remaining = DEBOUNCE.saturating_sub(dirty_at.elapsed());
    if remaining > Duration::ZERO {
        tx.wake_after(remaining);
        return;
    }
    // Files without a scanner language render plain text anyway; the view's
    // notice bar already says so — skip the worker round-trip entirely.
    if doc.code.language().is_none() {
        doc.highlight_dirty_at = None;
        return;
    }
    if doc.highlight_inflight {
        // The in-flight job's reply wakes the loop; the still-set
        // `highlight_dirty_at` then re-arms for the newer revision.
        return;
    }
    let (path, revision, text) = (
        doc.path.clone(),
        doc.code.revision,
        doc.code.edit.text.clone(),
    );
    doc.highlight_dirty_at = None;
    doc.highlight_inflight = true;
    let (Some(checkout), Some(key)) = (ed.checkout.clone(), ed.checkout_key.clone()) else {
        // A doc without checkout identity cannot happen through install_doc,
        // but never wedge the flag if state is ever half-built.
        if let Some(d) = ed.doc.as_mut() {
            d.highlight_dirty_at = Some(Instant::now());
            d.highlight_inflight = false;
        }
        return;
    };
    let ok = ed.enqueue_read(
        checkout,
        key,
        Req::Highlight {
            path,
            revision,
            text,
        },
    );
    if !ok {
        // Queue refused: retry on the next wake instead of losing colours.
        if let Some(d) = ed.doc.as_mut() {
            d.highlight_dirty_at = Some(Instant::now());
            d.highlight_inflight = false;
        }
    }
}

/// Apply a worker highlight reply. The backend already dropped replies for
/// a foreign checkout or a superseded generation; here the doc identity
/// (path) and the document revision do the final check — a result computed
/// for older text is discarded, not stored.
pub fn apply(ed: &mut EditorState, path: &str, result: HighlightResult) {
    let Some(doc) = ed.doc.as_mut() else {
        return;
    };
    if doc.path != path {
        return; // reply belongs to a since-replaced document
    }
    doc.highlight_inflight = false;
    if doc.code.revision == result.revision {
        doc.code.syntax = Some(result);
    }
}

/// A failed highlight job must not wedge `highlight_inflight` — the doc
/// simply stays uncoloured until the next edit re-arms the debounce.
pub fn apply_error(ed: &mut EditorState, path: &str) {
    if let Some(doc) = ed.doc.as_mut()
        && doc.path == path
    {
        doc.highlight_inflight = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::state::checkout_key;
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

    fn age_dirty(e: &mut EditorState) {
        e.doc.as_mut().unwrap().highlight_dirty_at =
            Some(Instant::now() - DEBOUNCE - Duration::from_millis(1));
    }

    #[test]
    fn debounce_waits_then_issues_once() {
        let mut e = state_with_doc();
        let mut tx = Tx::default();
        mark_edit(&mut e);
        // Inside the window: poll re-arms and does not enqueue.
        poll(&mut e, &mut tx);
        assert!(!e.doc.as_ref().unwrap().highlight_inflight);
        // Age the dirty mark past the debounce, then poll issues.
        age_dirty(&mut e);
        poll(&mut e, &mut tx);
        let doc = e.doc.as_ref().unwrap();
        assert!(doc.highlight_inflight);
        assert!(doc.highlight_dirty_at.is_none());
        // In-flight suppresses re-issue even if another edit lands.
        mark_edit(&mut e);
        age_dirty(&mut e);
        poll(&mut e, &mut tx);
        assert!(e.doc.as_ref().unwrap().highlight_inflight);
    }

    #[test]
    fn unknown_language_never_enqueues() {
        let mut e = state_with_doc();
        e.doc.as_mut().unwrap().path = "README".into();
        e.doc.as_mut().unwrap().code.path = "README".into();
        let mut tx = Tx::default();
        mark_edit(&mut e);
        age_dirty(&mut e);
        poll(&mut e, &mut tx);
        let doc = e.doc.as_ref().unwrap();
        assert!(doc.highlight_dirty_at.is_none());
        assert!(!doc.highlight_inflight);
    }

    #[test]
    fn apply_stores_only_matching_revision() {
        let mut e = state_with_doc();
        let rev = e.doc.as_ref().unwrap().code.revision;
        let mut result =
            crate::editor::syntax::highlight_revision(rev, Some("rust"), "fn main() {}");
        e.doc.as_mut().unwrap().highlight_inflight = true;
        apply(&mut e, "a.rs", result.clone());
        assert!(e.doc.as_ref().unwrap().code.syntax.is_some());
        assert!(!e.doc.as_ref().unwrap().highlight_inflight);
        // A stale-revision result for the same path is dropped.
        result.revision = rev + 99;
        e.doc.as_mut().unwrap().code.syntax = None;
        e.doc.as_mut().unwrap().highlight_inflight = true;
        apply(&mut e, "a.rs", result);
        assert!(e.doc.as_ref().unwrap().code.syntax.is_none());
        assert!(!e.doc.as_ref().unwrap().highlight_inflight);
        // A result for a different path touches nothing.
        e.doc.as_mut().unwrap().highlight_inflight = true;
        let fresh = crate::editor::syntax::highlight_revision(
            e.doc.as_ref().unwrap().code.revision,
            Some("rust"),
            "fn main() {}",
        );
        apply(&mut e, "other.rs", fresh);
        assert!(e.doc.as_ref().unwrap().highlight_inflight);
    }

    #[test]
    fn apply_error_clears_inflight_for_matching_path_only() {
        let mut e = state_with_doc();
        e.doc.as_mut().unwrap().highlight_inflight = true;
        apply_error(&mut e, "other.rs");
        assert!(e.doc.as_ref().unwrap().highlight_inflight);
        apply_error(&mut e, "a.rs");
        assert!(!e.doc.as_ref().unwrap().highlight_inflight);
    }

    #[test]
    fn discard_rearms_highlight() {
        let mut e = state_with_doc();
        e.doc.as_mut().unwrap().highlight_dirty_at = None;
        e.doc.as_mut().unwrap().discard();
        assert!(e.doc.as_ref().unwrap().highlight_dirty_at.is_some());
    }
}

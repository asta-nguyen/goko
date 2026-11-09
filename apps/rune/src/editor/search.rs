//! Content search for the Files section: a 250 ms debounce on the query
//! input, generation-tagged `Req::Search` reads through the shared backend,
//! and stale-reply protection via the request generation + query echo.
//!
//! `on_query_change` runs inside the TextInput `on_change` handler (has
//! `&mut State` + `&mut Tx`); `poll` runs from `AppSpec::poll` and issues
//! the pending query once the debounce wake arrives.

use rune::Tx;

use crate::State;
use crate::editor::backend::Req;

/// Minimum query length before a search is issued (matches the desktop).
pub const MIN_QUERY: usize = 2;

/// TextInput `on_change`: record the latest query and arm the debounce.
/// Every keystroke re-arms; `wake_after` schedules the poll that issues it,
/// so only the last query within the window is ever sent.
pub fn on_query_change(s: &mut State, tx: &mut Tx) {
    let query = s.editor.files.search_edit.text.trim().to_string();
    if query.chars().count() < MIN_QUERY {
        s.editor.files.search.pending = None;
        s.editor.files.search.results = None;
        s.editor.files.search.error = None;
        return;
    }
    s.editor.files.search.pending = Some((query, 0.0));
    tx.wake_after(std::time::Duration::from_millis(250));
}

/// `AppSpec::poll` hook: issue the debounced query if one is pending.
/// Returns `true` when visible state changed (a search was issued or
/// cleared), so the frame loop knows to redraw.
pub fn poll(s: &mut State) -> bool {
    let Some((query, _due)) = s.editor.files.search.pending.take() else {
        return false;
    };
    let Some(checkout) = s.editor.checkout.clone() else {
        return false;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return false;
    };
    // The query the user has since typed replaces this one; `pending` holds
    // only the latest, and `search.query` records what was actually issued
    // so apply_reply can drop superseded results.
    if s.editor.files.search_edit.text.trim() != query {
        // User kept typing during the debounce: re-arm instead of issuing.
        s.editor.files.search.pending =
            Some((s.editor.files.search_edit.text.trim().to_string(), 0.0));
        return true;
    }
    s.editor.files.search.query = query.clone();
    s.editor.files.search.searching = true;
    s.editor.files.search.error = None;
    s.editor.files.search.results = None;
    s.editor.enqueue_read(checkout, key, Req::Search { query });
    true
}

/// Clear search state on checkout switch (called from select_checkout path
/// via refresh; kept here so the section owns its reset).
pub fn reset(s: &mut State) {
    s.editor.files.search.pending = None;
    s.editor.files.search.results = None;
    s.editor.files.search.query.clear();
    s.editor.files.search.searching = false;
    s.editor.files.search.error = None;
    s.editor.files.search.checkout = s.editor.checkout_key.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::state::checkout_key;
    use std::path::PathBuf;

    fn state_with_checkout() -> State {
        let mut s = State::new(crate::test_handle());
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/tmp/co"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        s
    }

    /// The pending slot holds at most the latest query — a burst of
    /// `on_query_change` calls collapses to one issued search.
    #[test]
    fn debounce_keeps_only_latest_query() {
        let mut s = state_with_checkout();
        let mut tx = rune::Tx::default();
        s.editor.files.search_edit.text = "fir".into();
        on_query_change(&mut s, &mut tx);
        s.editor.files.search_edit.text = "first".into();
        on_query_change(&mut s, &mut tx);
        assert_eq!(
            s.editor
                .files
                .search
                .pending
                .as_ref()
                .map(|(q, _)| q.as_str()),
            Some("first")
        );
    }

    #[test]
    fn short_queries_never_issue() {
        let mut s = state_with_checkout();
        let mut tx = rune::Tx::default();
        s.editor.files.search_edit.text = "a".into();
        on_query_change(&mut s, &mut tx);
        assert!(s.editor.files.search.pending.is_none());
        assert!(s.editor.files.search.results.is_none());
        // Two chars meets the desktop's minimum and arms the debounce.
        s.editor.files.search_edit.text = "ab".into();
        on_query_change(&mut s, &mut tx);
        assert!(s.editor.files.search.pending.is_some());
    }

    #[test]
    fn poll_issues_pending_query_once() {
        let mut s = state_with_checkout();
        let mut tx = rune::Tx::default();
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s));
        assert!(s.editor.files.search.searching);
        assert_eq!(s.editor.files.search.query, "needle");
        // Nothing left pending: a second poll is a no-op.
        assert!(!poll(&mut s));
    }
}

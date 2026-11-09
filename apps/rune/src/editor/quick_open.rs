//! Cmd/Ctrl-P quick file picker: a modal palette over the cached
//! `files.paths` list (the same `Req::FileList` reply the Files tree uses —
//! no UI-thread reads, no new backend path).
//!
//! Opening routes every selection through `navigation::navigate` with
//! `PendingNav::OpenDoc`, so a dirty draft raises the existing
//! Save/Discard/Cancel guard. Matches are tagged with the checkout key they
//! were built from; a checkout switch or a refreshed path list invalidates
//! them instead of opening a stale path.

use rune::edit::TextEdit;
use rune::{El, Key, KeyEvent, Tx, col, el, row};

use crate::editor::files;
use crate::editor::navigation::{self, PendingNav};
use crate::editor::state::EditorState;
use crate::theme::{self, Ink};
use crate::{State, View};

/// Rows painted at once; the selection window scrolls inside the cap.
const MAX_VISIBLE: usize = 12;
/// Page Up/Down step, in rows.
const PAGE: isize = 8;
/// Stored match cap — the visible window is 12, this bounds sort cost on
/// very large trees without truncating what the user can realistically see.
const MAX_MATCHES: usize = 1000;

/// Palette state owned by `EditorState` (one per app, like the guard).
#[derive(Default)]
pub struct QuickOpenState {
    pub open: bool,
    /// Query draft; `focused` is set on open so the field owns typing.
    pub query: TextEdit,
    /// Index into `matches`.
    pub index: usize,
    /// Fuzzy-ranked checkout-relative paths for `query`.
    pub matches: Vec<String>,
    /// `files.paths.len()` the matches were built from.
    pub matched_len: usize,
    /// Checkout key the matches belong to.
    pub matched_checkout: Option<String>,
}

/// Open the palette: fresh query, focus inside the modal, matches for the
/// current (possibly empty) path cache. A missing cache asks the backend
/// for the file list through the normal Files refresh path.
pub fn open(s: &mut State) {
    if s.editor.quick_open.open {
        return;
    }
    crate::editor::keyboard::clear_edit_focuses(s);
    let qo = &mut s.editor.quick_open;
    qo.open = true;
    qo.query = TextEdit::new("");
    qo.query.focused = true;
    qo.index = 0;
    refresh_matches(&mut s.editor);
    // Cached paths may not have landed yet (fresh checkout): request the
    // list once through the existing backend read; `poll` rebuilds matches
    // when it arrives. Never fabricate an empty success.
    if s.editor.files.paths.is_empty() && s.editor.checkout_key.is_some() && !s.editor.files.loading
    {
        files::refresh_files(s);
    }
}

/// Close and reset; focus returns to whatever the section had (nothing is
/// re-focused — the next click/Tab owns it, same as the guard dialog).
pub fn close(s: &mut State) {
    s.editor.quick_open = QuickOpenState::default();
}

/// ⌘P / Ctrl-P toggle, called from `keyboard::on_key` while the palette is
/// closed. Returns `true` when the chord was ours.
pub fn chord(s: &mut State, ev: &KeyEvent) -> bool {
    if s.view != View::Editor || !ev.is_command('p') {
        return false;
    }
    if s.editor.quick_open.open {
        close(s);
    } else {
        open(s);
    }
    true
}

/// Key handling while the palette is open. The query `TextInput` sees keys
/// first through [`input_key`]; this is the app-level fallback for anything
/// the field declines (chords, function keys) plus the modal's own Escape.
/// Every key is consumed — the palette is modal.
pub fn key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.editor.quick_open.open {
        return false;
    }
    match &ev.key {
        Key::Escape => {
            close(s);
            true
        }
        Key::Up => {
            move_index(s, -1);
            true
        }
        Key::Down => {
            move_index(s, 1);
            true
        }
        Key::Home => {
            s.editor.quick_open.index = 0;
            true
        }
        Key::End => {
            let n = s.editor.quick_open.matches.len();
            s.editor.quick_open.index = n.saturating_sub(1);
            true
        }
        Key::PageUp => {
            move_index(s, -PAGE);
            true
        }
        Key::PageDown => {
            move_index(s, PAGE);
            true
        }
        Key::Enter => {
            activate_index(s, s.editor.quick_open.index, tx);
            true
        }
        _ => chord(s, ev),
    }
}

/// The query field's own `on_key`: same navigation as [`key`], but text
/// keys return `false` so `TextEdit` inserts them. Runs before the app-level
/// handler, so arrows/Enter/Escape are consumed here first.
pub fn input_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    key(s, ev, tx)
}

/// Rebuild `matches` when the path cache or checkout changed while open.
/// Called from `AppSpec::poll`; returns `true` when state changed.
pub fn poll(s: &mut State) -> bool {
    if !s.editor.quick_open.open {
        return false;
    }
    let qo = &s.editor.quick_open;
    let stale = qo.matched_checkout != s.editor.checkout_key
        || qo.matched_len != s.editor.files.paths.len();
    if stale {
        refresh_matches(&mut s.editor);
        return true;
    }
    false
}

fn move_index(s: &mut State, delta: isize) {
    let n = s.editor.quick_open.matches.len();
    if n == 0 {
        return;
    }
    let i = s.editor.quick_open.index as isize;
    s.editor.quick_open.index = (i + delta).clamp(0, n as isize - 1) as usize;
}

/// Open the match at `index` through the draft-guarded navigation path.
/// The palette closes first so the guard dialog (when it raises) is the
/// only modal on screen.
fn activate_index(s: &mut State, index: usize, tx: &mut Tx) {
    let Some(path) = s.editor.quick_open.matches.get(index).cloned() else {
        return;
    };
    close(s);
    navigation::navigate(s, PendingNav::OpenDoc { path, line: None }, tx);
}

/// Score `query` against `path`: a case-insensitive subsequence match with
/// bonuses for basename hits, consecutive runs and path-boundary starts.
/// `None` when the query isn't a subsequence. Public for tests.
pub fn score(query: &str, path: &str) -> Option<u32> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let p: Vec<char> = path.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(0);
    }
    let base = p.iter().rposition(|c| *c == '/').map_or(0, |i| i + 1);
    let mut qi = 0usize;
    let mut score = 0u32;
    let mut streak = 0u32;
    let mut last = usize::MAX;
    for (i, c) in p.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            // Boundary bonus: start of path, after '/', '.', '_' or '-'.
            let boundary =
                i == 0 || p[i - 1] == '/' || p[i - 1] == '.' || p[i - 1] == '_' || p[i - 1] == '-';
            let in_base = i >= base;
            score += 10 + u32::from(boundary) * 30 + u32::from(in_base) * 8 + streak.min(8) * 4;
            streak += 1;
            last = i;
            qi += 1;
        } else {
            streak = 0;
        }
    }
    if qi < q.len() {
        return None;
    }
    // Penalize spread: matches scattered across a long path rank lower.
    let span = last.saturating_sub(p.iter().position(|c| q.first() == Some(c)).unwrap_or(0)) as u32;
    score = score.saturating_sub(span / 8);
    Some(score)
}

/// Recompute `matches` from `files.paths` + `query`, best first. Takes the
/// editor state directly so the FileList reply arm can refresh an open
/// palette on a same-count path swap that `poll`'s len/checkout check
/// cannot see.
pub fn refresh_matches(e: &mut EditorState) {
    let qo = &mut e.quick_open;
    qo.matched_checkout = e.checkout_key.clone();
    qo.matched_len = e.files.paths.len();
    let query = qo.query.text.trim().to_string();
    let mut scored: Vec<(u32, &String)> = e
        .files
        .paths
        .iter()
        .filter_map(|p| score(&query, p).map(|sc| (sc, p)))
        .collect();
    // Empty query keeps backend (already sorted) order; otherwise rank by
    // score desc then path for determinism.
    if !query.is_empty() {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    }
    qo.matches = scored
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(_, p)| p.clone())
        .collect();
    qo.index = qo.index.min(qo.matches.len().saturating_sub(1));
}

/// Scrim + top-anchored layer, mirroring `rune_kit::Dialog::build`'s pair but
/// aligned to the top edge like a command palette. Mount as the last
/// children of the workspace root so they paint above everything.
pub fn layers(s: &State, ink: &Ink, k: &rune_kit::Kit) -> Vec<El> {
    if !s.editor.quick_open.open {
        return Vec::new();
    }
    let qo = &s.editor.quick_open;
    let scrim = el()
        .key("goko-quick-open-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(ink.bg.alpha(0.55))
        .a11y_hidden();

    let input = rune_kit::TextInput::new(&qo.query)
        .placeholder("Go to file…")
        .width(None)
        .on_edit(|st: &mut State| -> &mut TextEdit { &mut st.editor.quick_open.query })
        .on_key(input_key)
        .on_change(|st: &mut State, _tx: &mut Tx| {
            st.editor.quick_open.index = 0;
            refresh_matches(&mut st.editor);
        })
        .build(k);

    // Status line: honest about missing checkout / loading / errors — never
    // a fake "no results" while the path cache is still empty.
    let (status, tone) = if s.editor.checkout_key.is_none() {
        ("Select a project and checkout first".to_string(), ink.muted)
    } else if s.editor.files.error.is_some() {
        (s.editor.files.error.clone().unwrap_or_default(), ink.danger)
    } else if s.editor.files.paths.is_empty() {
        (
            if s.editor.files.loading {
                "Loading files…".to_string()
            } else {
                "No files".to_string()
            },
            ink.muted,
        )
    } else if qo.matches.is_empty() {
        ("No matching files".to_string(), ink.muted)
    } else {
        (
            format!(
                "{} file{}",
                qo.matches.len(),
                if qo.matches.len() == 1 { "" } else { "s" }
            ),
            ink.muted,
        )
    };

    // Visible window around the selection.
    let len = qo.matches.len();
    let start = if len <= MAX_VISIBLE || qo.index < MAX_VISIBLE / 2 {
        0
    } else if qo.index + MAX_VISIBLE / 2 >= len {
        len - MAX_VISIBLE
    } else {
        qo.index - MAX_VISIBLE / 2
    };
    let end = (start + MAX_VISIBLE).min(len);

    let mut results = col()
        .key("goko-quick-open-results")
        .role("listbox")
        .label("Matching files")
        .w_full()
        .gap(1.0);
    for (i, path) in qo.matches[start..end].iter().enumerate() {
        let idx = start + i;
        let selected = idx == qo.index;
        let row_key = format!("goko-quick-open-row:{path}");
        let mut r = row()
            .key(row_key.clone())
            .role("option")
            .label(path.clone())
            .w_full()
            .h(26.0)
            .px(10.0)
            .items_center()
            .gap(8.0)
            .radius(4.0)
            .hover(ink.text.alpha(0.06))
            .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                st.editor.quick_open.index = idx;
                activate_index(st, idx, tx);
            })
            .child(theme::txt(
                path,
                theme::T12,
                if selected { ink.text } else { ink.muted },
            ));
        if selected {
            r = r.fill(ink.text.alpha(0.10));
        }
        results = results.child(r);
    }

    let card = col()
        .key("goko-quick-open-card")
        .role("dialog")
        .label("Quick open")
        .w(560.0)
        .max_h(420.0)
        .fill(ink.panel)
        .border_sides([1.0, 1.0, 1.0, 1.0], ink.border)
        .radius(8.0)
        .padding(8.0)
        .gap(6.0)
        .child(input)
        .child(theme::txt(&status, theme::T11, tone).px(4.0))
        .child(results)
        .child(theme::txt("↑↓ select · ↵ open · esc close", theme::T11, ink.muted).px(4.0))
        .modal(true, close)
        .modal_click_outside(true);
    let card = if len > 0 {
        card.active_descendant(format!(
            "goko-quick-open-row:{}",
            qo.matches[qo.index.min(len - 1)]
        ))
    } else {
        card
    };

    let layer = col()
        .key("goko-quick-open-layer")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .items_center()
        .pt(48.0)
        .child(card);
    vec![scrim, layer]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, El, Env, Mods, Runtime, col};

    /// Mirrors `main.rs::on_key`'s palette routing for these tests.
    fn test_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if s.editor.quick_open.open {
            key(s, ev, tx)
        } else {
            chord(s, ev)
        }
    }

    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = std::path::PathBuf::from("/tmp/goko-quickopen-qa");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s
    }

    fn runtime() -> Runtime<State> {
        let mut rt = Runtime::new(scene(), view, (900.0, 700.0), Env::new(true, 2.0));
        rt.on_key = Some(test_key);
        rt
    }

    fn view(s: &State, _cx: &mut Cx<State>) -> El {
        let mut root = col().grow(1.0).h_full();
        for l in layers(s, &s.ink, &s.editor.kit) {
            root = root.child(l);
        }
        root
    }

    fn text_key(t: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Text(t.to_string()),
            mods: Mods::default(),
        }
    }

    fn cmd_key(c: char) -> KeyEvent {
        KeyEvent {
            key: Key::Command(c),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        }
    }

    #[test]
    fn score_ranks_filename_and_boundary_hits() {
        assert!(score("main", "src/main.rs").is_some());
        assert!(score("mnr", "src/main.rs").is_some(), "subsequence matches");
        assert!(score("zzz", "src/main.rs").is_none());
        assert!(score("", "anything").is_some());
        // Boundary/consecutive/basename hits outrank scattered non-boundary
        // matches.
        let strong = score("mod", "src/models/mod.rs").unwrap();
        let weak = score("mod", "src/xmxoxd.rs").unwrap();
        assert!(strong > weak, "{strong} vs {weak}");
    }

    #[test]
    fn score_handles_unicode() {
        assert!(score("héllo", "src/héllo.rs").is_some());
        assert!(score("", "docs/.md").is_some());
        assert!(score("zzz", "docs/english.md").is_none());
        // Case folding across scripts.
        assert!(score("ÄFFÄR", "src/äffär.rs").is_some());
    }

    #[test]
    fn chord_opens_and_closes_palette() {
        let mut rt = runtime();
        rt.frame(0.0);
        assert!(!rt.state.editor.quick_open.open);
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        assert!(rt.state.editor.quick_open.open);
        assert!(rt.state.editor.quick_open.query.focused);
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        assert!(!rt.state.editor.quick_open.open);
    }

    #[test]
    fn typing_query_filters_matches() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec![
            "src/main.rs".into(),
            "src/lib.rs".into(),
            "docs/guide.md".into(),
        ];
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        for ch in "main".chars() {
            rt.key(text_key(&ch.to_string()));
        }
        rt.frame(0.0);
        assert_eq!(rt.state.editor.quick_open.matches, vec!["src/main.rs"]);
        assert_eq!(rt.state.editor.quick_open.index, 0);
    }

    #[test]
    fn arrows_move_selection_and_enter_activates() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec!["a.rs".into(), "b.rs".into(), "c.rs".into()];
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.editor.quick_open.index, 1);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.editor.quick_open.open, "palette closed on open");
    }

    #[test]
    fn escape_closes_without_navigating() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec!["a.rs".into()];
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.editor.quick_open.open);
    }

    #[test]
    fn empty_path_cache_shows_no_fake_results() {
        let mut rt = runtime();
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        assert!(rt.state.editor.quick_open.matches.is_empty());
    }

    #[test]
    fn enter_with_dirty_doc_raises_save_guard() {
        use crate::editor::draft::OpenDoc;
        let mut rt = runtime();
        // Dirty doc on disk-different text: opening another file must raise
        // the existing Save/Discard/Cancel guard, not navigate silently.
        let file = app_core::git_view::GitViewFile {
            path: "src/keep.rs".into(),
            kind: "text",
            text: Some("fn keep() {}\n".into()),
            truncated: false,
            size: 13,
        };
        let mut doc = OpenDoc::new("src/keep.rs", &file);
        doc.code.edit.text.push_str("// unsaved\n");
        assert!(doc.dirty());
        rt.state.editor.doc = Some(doc);
        rt.state.editor.files.paths = vec!["a.rs".into(), "b.rs".into()];
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.editor.quick_open.open, "palette closed");
        let guard = rt
            .state
            .editor
            .guard
            .as_ref()
            .expect("dirty draft raised the guard");
        assert!(
            matches!(guard.pending, PendingNav::OpenDoc { .. }),
            "guard carries the OpenDoc navigation"
        );
    }

    #[test]
    fn checkout_change_invalidates_matches() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec!["a.rs".into()];
        rt.key(cmd_key('p'));
        rt.frame(0.0);
        assert_eq!(rt.state.editor.quick_open.matches.len(), 1);
        // A checkout switch clears the path cache (select_checkout); the
        // palette must not keep offering the old checkout's files.
        rt.state.editor.checkout_key = Some("other".into());
        rt.state.editor.files.paths.clear();
        assert!(poll(&mut rt.state), "poll detects stale matches");
        assert!(rt.state.editor.quick_open.matches.is_empty());
        assert_eq!(
            rt.state.editor.quick_open.matched_checkout.as_deref(),
            Some("other")
        );
    }
}

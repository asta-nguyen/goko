//! Shared patch pane: one renderer for the Changes per-file diff, the
//! History commit diff and the Stashes stash diff. It binds the real
//! `rune_kit::diff::DiffState`/`DiffView` (unified/split, folding, keyboard
//! navigation, selection, syntax highlighting) — never a custom diff
//! renderer — and layers the binary/truncated/empty notices plus the
//! changed-file name list on top, matching the desktop `DiffPatch`.
//!
//! The `DiffState` for each pane lives in `EditorState`
//! (`changes.diff` / `history.diff` / `stashes.diff`); the backend parses
//! `patch.raw` into it via `diff::parse` + `set_files` when the matching
//! reply lands. This module only reads it.

use rune::{El, Tx, col, row};
use rune_kit::diff::{DiffMode, DiffState, DiffView};
use rune_kit::{Kit, SegmentedControl};

use crate::State;
use crate::editor::state::Patch;
use crate::theme::{self, Ink, T11M_LH, T12, T12M, T12TN};

/// Which section's diff the pane renders.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Pane {
    Changes,
    History,
    Stashes,
}

impl Pane {
    pub fn label(self) -> &'static str {
        match self {
            Pane::Changes => "changes",
            Pane::History => "history",
            Pane::Stashes => "stashes",
        }
    }
    /// Empty-state hint when nothing is selected/loaded yet.
    fn empty_hint(self) -> &'static str {
        match self {
            Pane::Changes => "Select a changed file to see its diff",
            Pane::History => "Select a commit to see its diff",
            Pane::Stashes => "Select a stash to see its diff",
        }
    }
    fn loading_label(self) -> &'static str {
        match self {
            Pane::Changes => "Loading diff…",
            Pane::History => "Loading commit…",
            Pane::Stashes => "Loading stash…",
        }
    }
}

// ---------------------------------------------------------------------------
// State accessors (seam: `*.diff` DiffState fields are added to EditorState
// by the integrator; backends fill them with `set_files(parse(&patch.raw))`)
// ---------------------------------------------------------------------------

fn diff_state(s: &State, pane: Pane) -> &DiffState {
    match pane {
        Pane::Changes => &s.editor.changes.diff,
        Pane::History => &s.editor.history.diff,
        Pane::Stashes => &s.editor.stashes.diff,
    }
}

fn diff_state_mut(s: &mut State, pane: Pane) -> &mut DiffState {
    match pane {
        Pane::Changes => &mut s.editor.changes.diff,
        Pane::History => &mut s.editor.history.diff,
        Pane::Stashes => &mut s.editor.stashes.diff,
    }
}

/// A patch only renders while it belongs to the current checkout: the
/// backend stamps `Patch.checkout` with the checkout key that produced it,
/// so a reply landing after a checkout switch (or any stale slot) shows the
/// empty state instead of another checkout's diff.
fn patch_fresh(patch: &Patch, checkout_key: Option<&str>) -> bool {
    checkout_key == Some(patch.checkout.as_str())
}

fn patch_for(s: &State, pane: Pane) -> Option<&Patch> {
    let key = s.editor.checkout_key.as_deref();
    match pane {
        Pane::Changes => s
            .editor
            .changes
            .patch
            .as_ref()
            .filter(|p| patch_fresh(p, key)),
        Pane::History => s
            .editor
            .history
            .detail
            .as_ref()
            .filter(|p| patch_fresh(p, key)),
        Pane::Stashes => s
            .editor
            .stashes
            .detail
            .as_ref()
            .filter(|p| patch_fresh(p, key)),
    }
}

fn patch_loading(s: &State, pane: Pane) -> bool {
    match pane {
        Pane::Changes => s.editor.changes.patch_loading,
        Pane::History => s.editor.history.detail_loading,
        Pane::Stashes => s.editor.stashes.detail_loading,
    }
}

/// Title shown at the left of the toolbar: the selected file/commit/stash.
fn title_for(s: &State, pane: Pane) -> String {
    match pane {
        Pane::Changes => s
            .editor
            .changes
            .patch_path
            .clone()
            .or_else(|| s.editor.changes.selected.clone())
            .unwrap_or_default(),
        Pane::History => s
            .editor
            .history
            .selected
            .clone()
            .map(|id| id.chars().take(10).collect())
            .unwrap_or_default(),
        Pane::Stashes => s.editor.stashes.selected.clone().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Pure notice helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Banner kinds the pane can show, in render order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notice {
    Binary,
    Truncated,
}

/// Which notice banners apply to a loaded patch.
pub fn notices(patch: &Patch) -> Vec<Notice> {
    let mut out = Vec::new();
    if patch.binary {
        out.push(Notice::Binary);
    }
    if patch.truncated {
        out.push(Notice::Truncated);
    }
    out
}

/// Whether the changed-file name list should render instead of the diff:
/// the parsed `DiffState` is empty but the patch still names files (binary
/// diffs, merge commits, or content the parser can't represent).
pub fn show_file_names(diff_files: usize, patch_files: usize) -> bool {
    diff_files == 0 && patch_files > 0
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The reusable patch pane for `pane`. Renders its own toolbar (title +
/// diff label + unified/split control), the binary/truncated banners, then
/// the real `DiffView` — or the loading/empty/changed-names fallbacks.
pub fn view(s: &State, pane: Pane) -> El {
    let ink = Ink::new();
    let k = &s.editor.kit;
    let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0).w_full();
    root = root.child(toolbar(s, pane, &ink, k));
    if let Some(patch) = patch_for(s, pane) {
        for notice in notices(patch) {
            root = root.child(notice_bar(notice, &ink));
        }
    }
    // `DiffView` renders but does not bind keys itself (the kit example
    // routes them); scope `DiffState::key` to the body so diff navigation
    // (j/k, hunk expand, mode keys) works without touching the toolbar.
    let body_el = body(s, pane, &ink, k).grow(1.0).min_h(0.0).on_key(
        move |st: &mut State, ev: &rune::KeyEvent, tx: &mut Tx| {
            diff_state_mut(st, pane).key(ev, tx)
        },
    );
    root.child(body_el)
}

fn toolbar(s: &State, pane: Pane, ink: &Ink, k: &Kit) -> El {
    let title = title_for(s, pane);
    let label = patch_for(s, pane)
        .map(|p| p.label.clone())
        .unwrap_or_default();
    let mode = diff_state(s, pane).mode();
    let selected = if mode == DiffMode::Split {
        Some(1)
    } else {
        Some(0)
    };
    row()
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .child(
            theme::txt(&title, T12M, ink.text)
                .min_w(0.0)
                .grow(1.0)
                .truncate(),
        )
        // `min_w(0)` lets the label compress to nothing before the fixed
        // mode control is pushed out of a narrow patch pane.
        .child(
            theme::txt(&label, T11M_LH, ink.muted)
                .min_w(0.0)
                .max_w(160.0)
                .truncate(),
        )
        .child(
            SegmentedControl::new(&["Unified", "Split"], selected)
                .fit()
                .label("Diff view")
                .on_pick(move |st: &mut State, i: usize, _tx: &mut Tx| {
                    diff_state_mut(st, pane).set_mode(if i == 1 {
                        DiffMode::Split
                    } else {
                        DiffMode::Unified
                    });
                })
                .build(k),
        )
}

fn notice_bar(notice: Notice, ink: &Ink) -> El {
    let (text, color) = match notice {
        Notice::Binary => ("Binary file — no textual diff", ink.warning),
        Notice::Truncated => ("Diff truncated at the 1 MiB cap", ink.warning),
    };
    row()
        .w_full()
        .px(10.0)
        .py(4.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .child(theme::txt(text, T11M_LH, color))
}

fn body(s: &State, pane: Pane, ink: &Ink, k: &Kit) -> El {
    if patch_loading(s, pane) && patch_for(s, pane).is_none() {
        return centered(ink, pane.loading_label());
    }
    let Some(patch) = patch_for(s, pane) else {
        return centered(ink, pane.empty_hint());
    };
    let files_in_diff = diff_state(s, pane).files().len();
    if show_file_names(files_in_diff, patch.files.len()) {
        return file_names(patch, ink);
    }
    if files_in_diff == 0 {
        return centered(ink, "No diff to show");
    }
    diff_view(s, pane, k)
}

fn centered(ink: &Ink, label: &str) -> El {
    col()
        .grow(1.0)
        .w_full()
        .items_center()
        .justify_center()
        .child(theme::txt(label, T12, ink.muted))
}

/// Changed-file names for diffs the parser can't render (binary, merges).
fn file_names(patch: &Patch, ink: &Ink) -> El {
    let mut list = col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .px(10.0)
        .py(8.0)
        .gap(2.0);
    list = list.child(theme::txt(
        &format!(
            "{} changed file{}",
            patch.files.len(),
            if patch.files.len() == 1 { "" } else { "s" }
        ),
        T11M_LH,
        ink.muted,
    ));
    for f in patch.files.iter().take(500) {
        let name = match &f.orig_path {
            Some(orig) => format!("{orig} → {}", f.path),
            None => f.path.clone(),
        };
        list = list.child(
            row()
                .w_full()
                .px(6.0)
                .py(2.0)
                .gap(8.0)
                .items_center()
                .child(
                    theme::txt(&name, T12, ink.text)
                        .min_w(0.0)
                        .grow(1.0)
                        .truncate(),
                )
                .child(theme::txt(f.status, T12TN, ink.muted)),
        );
    }
    col()
        .child(list)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0)
        .w_full()
}

/// The real `DiffView`, keyed per pane, with `DiffState::key` wired for
/// hunk/file navigation (j/k, ]/[, expand, mode keys) like the kit example.
fn diff_view(s: &State, pane: Pane, k: &Kit) -> El {
    DiffView::new(
        move |st: &State| diff_state(st, pane),
        move |st: &mut State| diff_state_mut(st, pane),
    )
    .key(format!("patch:{}", pane.label()))
    .build(s, k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::git_view::StatusEntry;

    fn patch(binary: bool, truncated: bool, files: usize) -> Patch {
        Patch {
            label: "test".into(),
            raw: String::new(),
            truncated,
            binary,
            files: (0..files)
                .map(|i| StatusEntry {
                    path: format!("f{i}"),
                    orig_path: None,
                    status: "modified",
                    added: None,
                    removed: None,
                })
                .collect(),
            detail: None,
            checkout: "ck".into(),
        }
    }

    #[test]
    fn notices_order_binary_then_truncated() {
        assert_eq!(
            notices(&patch(true, true, 1)),
            vec![Notice::Binary, Notice::Truncated]
        );
        assert_eq!(notices(&patch(false, true, 1)), vec![Notice::Truncated]);
        assert!(notices(&patch(false, false, 1)).is_empty());
    }

    #[test]
    fn file_names_only_when_diff_empty_but_files_known() {
        assert!(show_file_names(0, 3));
        assert!(!show_file_names(0, 0));
        assert!(!show_file_names(2, 3));
    }

    #[test]
    fn pane_labels_and_hints() {
        assert_eq!(Pane::Changes.label(), "changes");
        assert_eq!(
            Pane::History.empty_hint(),
            "Select a commit to see its diff"
        );
        assert_eq!(Pane::Stashes.loading_label(), "Loading stash…");
    }

    // -- provenance: a patch only renders for the checkout that fetched it --

    use std::path::PathBuf;

    /// Restores `GOKO_HOME` on drop (same guard the other editor tests use).
    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os("GOKO_HOME");
            // SAFETY: test-only; the pointed-at directory outlives the test.
            unsafe { std::env::set_var("GOKO_HOME", path) };
            Self(prev)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: same test-only env window as `set`.
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("GOKO_HOME", v),
                    None => std::env::remove_var("GOKO_HOME"),
                }
            }
        }
    }

    /// Wrong-checkout regression: `patch_for` must hide a stored patch whose
    /// `checkout` key is not the editor's current one, for every pane.
    #[test]
    fn patch_for_hides_patches_from_other_checkouts() {
        let home = app_core::test_support::TempDir::new("patch-prov");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/tmp/co-a"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/tmp/co-a"),
        ));

        let mut foreign = patch(false, false, 1);
        foreign.checkout = "other-checkout-key".into();
        s.editor.changes.patch = Some(foreign.clone());
        s.editor.history.detail = Some(foreign.clone());
        s.editor.stashes.detail = Some(foreign);

        assert!(patch_for(&s, Pane::Changes).is_none(), "changes");
        assert!(patch_for(&s, Pane::History).is_none(), "history");
        assert!(patch_for(&s, Pane::Stashes).is_none(), "stashes");

        // Same patch re-stamped with the current key renders again.
        for p in [
            &mut s.editor.changes.patch,
            &mut s.editor.history.detail,
            &mut s.editor.stashes.detail,
        ] {
            if let Some(p) = p.as_mut() {
                p.checkout = s.editor.checkout_key.clone().unwrap();
            }
        }
        assert!(patch_for(&s, Pane::Changes).is_some());
        assert!(patch_for(&s, Pane::History).is_some());
        assert!(patch_for(&s, Pane::Stashes).is_some());

        // No checkout at all hides even an otherwise matching patch.
        s.editor.checkout_key = None;
        assert!(patch_for(&s, Pane::Changes).is_none());
    }
}

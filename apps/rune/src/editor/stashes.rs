//! Native Stashes section: repository-wide stash list with filter, selection,
//! metadata header, changed files, and the shared `patch` diff pane. Ports the
//! desktop `EditorStashes` — no stash write actions.

use rune::{El, ScrollSpec, TextStyle, Tx, col, el, row};
use rune_kit::Kit;

use crate::State;
use crate::editor::backend::Req;
use crate::editor::commit_detail::changed_files;
use crate::editor::history::{self, format_time, matches_query};
use crate::editor::history_layout::PaneKind;
use crate::editor::keyboard;
use crate::editor::pane_layout;
use crate::editor::patch::{self, Pane};
use crate::theme::{BOOK_OPEN, Ink, T11, T11MN_BADGE, T11ST, T12, T12M, btn_sm, ic, txt};

/// `font-mono text-xs font-medium` — the `stash@{n}` label (desktop list row
/// and detail header).
const T12MONO_M: TextStyle = TextStyle::new(12.0, 500.0).mono();

/// Required public entry: renders Stashes from shared state.
pub fn view(s: &State) -> El {
    stashes_view_impl(s, &s.ink, &s.editor.kit)
}

fn stashes_view_impl(s: &State, ink: &Ink, k: &Kit) -> El {
    let h = &s.editor.stashes;

    if h.loading && h.stashes.is_empty() {
        return history::loading_pane("Loading stashes…", ink);
    }
    if h.error.is_some() && h.stashes.is_empty() {
        return history::error_pane(h.error.as_deref().unwrap_or("failed"), ink);
    }

    let mut root = row().grow(1.0).min_w(0.0).h_full().min_h(0.0);
    root = root.child(
        aside(s, ink, k)
            .w(pane_layout::current(s, PaneKind::StashesList))
            .h_full()
            .shrink()
            .min_w(pane_layout::PANE_MIN)
            .border_sides([0.0, 1.0, 0.0, 0.0], ink.border),
    );
    root = root.child(
        detail_pane(s, ink, k)
            .grow(1.0)
            .min_w(pane_layout::SIBLING_MIN)
            .h_full(),
    );

    // Auto-select the newest stash once data arrives (desktop effect); the
    // same callback records the split row's width for pane drag clamping.
    root = root.on_layout_tx(|st: &mut State, r: [f32; 4], _tx: &mut Tx| {
        st.editor.pane_row_w = r[2];
        let empty = st.editor.stashes.selected.is_none() && !st.editor.stashes.stashes.is_empty();
        if empty {
            let id = st.editor.stashes.stashes[0].id.clone();
            st.editor.stashes.selected = Some(id.clone());
            st.editor.stashes.detail = None;
            enqueue_stash_diff(st, &id);
        }
    });
    root
}

fn enqueue_stash_diff(s: &mut State, id: &str) {
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    s.editor.stashes.detail_loading = true;
    if !s.editor.enqueue_read(
        checkout,
        key,
        Req::StashDiff {
            stash: id.to_string(),
        },
    ) {
        s.editor.stashes.detail_loading = false;
    }
}

fn enqueue_stashes_refresh(s: &mut State) {
    if let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    {
        s.editor.stashes.loading = true;
        s.editor.stashes.error = None;
        s.editor
            .enqueue_read(checkout.clone(), key.clone(), Req::Stashes);
        if let Some(id) = s.editor.stashes.selected.clone() {
            enqueue_stash_diff(s, &id);
        }
    }
}

fn aside(s: &State, ink: &Ink, k: &Kit) -> El {
    let h = &s.editor.stashes;
    // Desktop `SplitPane` aside: `bg-panel/40` behind the list column.
    let mut col_el = col()
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(ink.panel.alpha(0.4));

    // Header: muted semibold uppercase title + refresh (desktop `text-[11px]
    // font-semibold uppercase tracking-wider text-muted`).
    col_el = col_el.child(
        row()
            .w_full()
            .items_center()
            .px(8.0)
            .py(6.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(txt("Stashes", T11ST, ink.muted).text_case(rune::TextCase::Uppercase))
            .child(el().grow(1.0))
            .child(
                btn_sm("Refresh", None, false, ink)
                    .label("Refresh stashes")
                    .on_press_tx(|st: &mut State, _tx: &mut Tx| enqueue_stashes_refresh(st)),
            ),
    );
    // Desktop shows this unconditionally — the stash stack is repository-wide.
    col_el = col_el.child(
        txt(
            "Shared across all worktrees of this repository.",
            T11,
            ink.muted,
        )
        .px(12.0)
        .py(6.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border),
    );

    // Filter.
    col_el = col_el.child(
        row()
            .w_full()
            .px(8.0)
            .py(4.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(
                rune_kit::TextInput::new(&h.filter)
                    .placeholder("Filter stashes")
                    .width(None)
                    .on_edit(|st: &mut State| -> &mut rune::edit::TextEdit {
                        &mut st.editor.stashes.filter
                    })
                    .build(k),
            ),
    );

    // List.
    let q = h.filter.text.trim().to_string();
    let visible: Vec<&app_core::git_view::GitViewStash> = if q.is_empty() {
        h.stashes.iter().collect()
    } else {
        h.stashes
            .iter()
            .filter(|it| {
                matches_query(
                    &format!("{} {}", it.message, it.branch.as_deref().unwrap_or("")),
                    &q,
                )
            })
            .collect()
    };

    let mut list = col()
        .grow(1.0)
        .min_h(0.0)
        .key(keyboard::nav_scroll_scope(
            crate::editor::state::Section::Stashes,
        ))
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .px(6.0)
        .py(6.0)
        .gap(2.0);
    if visible.is_empty() {
        list = list.child(
            txt(
                if q.is_empty() {
                    "No stashes in this repository."
                } else {
                    "No stashes match."
                },
                T12,
                ink.muted,
            )
            .px(8.0)
            .py(12.0),
        );
    }
    for item in visible {
        let selected = h.selected.as_deref() == Some(item.id.as_str());
        let oid = item.id.clone();
        let nav_key = keyboard::stash_row_key(&item.id);
        let nav_cur = keyboard::nav_active(s, crate::editor::state::Section::Stashes, &nav_key);
        // Desktop row: `flex-col gap-0.5 rounded px-2 py-1.5`, selected
        // `bg-secondary/70`, hover `bg-bg/60`.
        let mut btn = col()
            .w_full()
            .key(nav_key)
            .gap(2.0)
            .px(8.0)
            .py(6.0)
            .radius(4.0)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .child(txt(
                        &format!("stash@{{{}}}", item.index),
                        T12MONO_M,
                        ink.text,
                    ))
                    .child(if let Some(b) = &item.branch {
                        el().px(6.0)
                            .h(20.0)
                            .radius(10.0)
                            .items_center()
                            .justify_center()
                            .border(1.0, ink.border)
                            .label(b)
                            .child(txt(b, T11MN_BADGE, ink.muted))
                    } else {
                        el()
                    })
                    .child(el().grow(1.0))
                    .child(txt(&format_time(item.time), T11, ink.muted)),
            )
            .child(txt(&item.message, T12, ink.muted).w_full().truncate());
        btn = if nav_cur {
            btn.fill(ink.accent.alpha(0.14))
        } else if selected {
            btn.fill(ink.panel.alpha(0.7))
        } else {
            btn.hover(ink.bg.alpha(0.6))
        };
        btn = btn
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                if st.editor.stashes.selected.as_deref() == Some(oid.as_str()) {
                    return;
                }
                st.editor.stashes.selected = Some(oid.clone());
                st.editor.stashes.detail = None;
                enqueue_stash_diff(st, &oid);
            });
        list = list.child(btn);
    }
    col_el.child(list).child(pane_layout::grip(
        s,
        ink,
        PaneKind::StashesList,
        "Resize stash list and detail",
    ))
}

fn detail_pane(s: &State, ink: &Ink, _k: &Kit) -> El {
    let h = &s.editor.stashes;
    let Some(id) = h.selected.clone() else {
        return history::empty_pane(
            ink,
            "Select a stash to see its patch",
            "The stash stack is shared by every worktree of this repository.",
        );
    };
    let stash = h.stashes.iter().find(|it| it.id == id);
    if h.detail.is_some() {
        let mut col_el = col().grow(1.0).min_h(0.0);
        col_el = col_el.child(stash_header(s, ink, stash));
        if let Some(p) = &h.detail
            && !p.files.is_empty()
        {
            col_el = col_el.child(changed_files(&p.files, ink));
        }
        col_el.child(patch::view(s, Pane::Stashes).grow(1.0).min_h(0.0))
    } else if h.detail_loading {
        history::loading_pane("Loading stash…", ink)
    } else {
        // Diff request failed (backend surfaces the error as a global notice);
        // offer a retry like the desktop ErrorPane does.
        let oid = id.clone();
        col()
            .grow(1.0)
            .min_h(0.0)
            .items_center()
            .justify_center()
            .gap(8.0)
            .child(txt("Couldn't load this stash.", T12, ink.muted))
            .child(btn_sm("Retry", None, false, ink).on_press_tx(
                move |st: &mut State, _tx: &mut Tx| {
                    st.editor.stashes.detail = None;
                    enqueue_stash_diff(st, &oid);
                },
            ))
    }
}

fn stash_header(s: &State, ink: &Ink, stash: Option<&app_core::git_view::GitViewStash>) -> El {
    let _ = s;
    let mut header = col()
        .w_full()
        .px(12.0)
        .py(6.0)
        .gap(4.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
    // Desktop: archive icon + `stash@{n}` in mono medium followed by the
    // message in the same truncating span, a secondary `branch:` badge and
    // the timestamp.
    let mut top = row().w_full().items_center().gap(8.0);
    top = top.child(ic(&BOOK_OPEN, 14.0, ink.muted));
    let label = stash
        .map(|it| format!("stash@{{{}}}", it.index))
        .unwrap_or_else(|| "stash".into());
    let msg = stash.map(|it| it.message.clone()).unwrap_or_default();
    let mut title = row().min_w(0.0).grow(1.0).items_center().gap(0.0);
    title = title.child(txt(&label, T12MONO_M, ink.text));
    if !msg.is_empty() {
        title = title.child(
            txt(&format!(" {msg}"), T12M, ink.text)
                .min_w(0.0)
                .grow(1.0)
                .truncate(),
        );
    }
    top = top.child(title);
    if let Some(b) = stash.and_then(|it| it.branch.as_ref()) {
        top = top.child(
            el().px(6.0)
                .h(20.0)
                .radius(10.0)
                .items_center()
                .justify_center()
                .fill(ink.panel)
                .label(format!("branch: {b}"))
                .child(txt(&format!("branch: {b}"), T11MN_BADGE, ink.text)),
        );
    }
    if let Some(t) = stash.map(|it| it.time) {
        top = top.child(txt(&format_time(t), T12, ink.muted));
    }
    header = header.child(top);
    header
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::View;
    use crate::editor::state::Section;
    use app_core::git_view::GitViewStash;
    use rune::{Env, Runtime};

    /// Restores `GOKO_HOME` on drop so the throwaway home never leaks into
    /// parallel tests (same guard the changes-scene tests use).
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

    #[test]
    fn stash_filter_uses_message_and_branch() {
        let msg = "WIP on main: abc1234 stuff";
        assert!(matches_query(&format!("{msg} main"), "wip main"));
        assert!(!matches_query(&format!("{msg} main"), "wip develop"));
    }

    #[test]
    fn stash_label_format() {
        assert_eq!(format!("stash@{{{}}}", 3usize), "stash@{3}");
    }

    /// CUA repro for the inert filter: `TextInput::new` without `.on_edit`
    /// left `stashes.filter` read-only. Click the field, type, and the parent
    /// `TextEdit` plus the visible rows must follow; backspacing restores.
    #[test]
    fn filter_input_types_and_hides_rows() {
        let home = app_core::test_support::TempDir::new("stashes-filter-qa");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = std::path::PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = Section::Stashes;
        s.editor.stashes.stashes = vec![
            GitViewStash {
                index: 0,
                id: "stash0".into(),
                message: "WIP on main: fix login".into(),
                branch: Some("main".into()),
                time: 1_700_000_000_000,
            },
            GitViewStash {
                index: 1,
                id: "stash1".into(),
                message: "WIP on dev: add tests".into(),
                branch: Some("dev".into()),
                time: 1_699_000_000_000,
            },
        ];
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        // The only textbox in the Stashes view is the list filter.
        let tb = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "textbox")
            .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
            .expect("filter textbox mounted");
        let r = tb.rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        for ch in "fix".chars() {
            rt.key(rune::KeyEvent {
                key: rune::Key::Text(ch.to_string()),
                mods: rune::Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.editor.stashes.filter.text, "fix",
            "typing must reach stashes.filter"
        );
        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(
            texts.iter().any(|t| t.contains("fix login")),
            "matching stash stays: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("add tests")),
            "non-matching stash hides: {texts:?}"
        );

        for _ in 0..3 {
            rt.key(rune::KeyEvent {
                key: rune::Key::Backspace,
                mods: rune::Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.editor.stashes.filter.text, "");
        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(
            texts.iter().any(|t| t.contains("add tests")),
            "cleared filter restores rows: {texts:?}"
        );
    }

    /// The stash list/detail split exposes the draggable separator; dragging
    /// persists `stashesList` through the shared layout lane.
    #[test]
    fn pane_grip_drags_stashes_list() {
        let home = app_core::test_support::TempDir::new("stashes-pane-qa");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = std::path::PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = Section::Stashes;
        s.editor.stashes.stashes = vec![GitViewStash {
            index: 0,
            id: "stash0".into(),
            message: "WIP on main: fix login".into(),
            branch: Some("main".into()),
            time: 1_700_000_000_000,
        }];
        let mut rt = Runtime::new(s, crate::view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let g = rt
            .elements()
            .into_iter()
            .find(|e| {
                e.role == "separator" && e.label.as_deref() == Some("Resize stash list and detail")
            })
            .expect("stashes pane grip mounted");
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        for i in 1..=4 {
            rt.pointer_move(x - 40.0 * i as f32 / 4.0, y);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x - 40.0, y);
        rt.frame(1.0 / 60.0);

        let w = rt.state.editor.history.layout.panes.stashes_list;
        assert!((w - 240.0).abs() < 1.0, "drag -40px: 280 -> {w}");
        assert!(rt.state.editor.history.layout_dirty);
    }
}

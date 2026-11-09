//! Changes section: the checkout's staged/unstaged/untracked/conflict groups
//! with per-row and per-group Git writes, the discard confirmation, the
//! changed-file filter, and the shared patch pane (`patch::view`).
//!
//! Mirrors the desktop `EditorChanges.tsx`: groups keep main's order
//! (conflicts → staged → unstaged → untracked), a row click only selects the
//! diff (action buttons are siblings, never nested inside the select row),
//! "Stage all" scopes to that group's paths while "Unstage all" sends
//! `paths: None`, and discard is a named one-file confirmation.

use rune::{El, Icon, Tx, col, el, row};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::history_layout::PaneKind;
use crate::editor::keyboard;
use crate::editor::pane_layout;
use crate::editor::patch::{self, Pane};
use crate::editor::state::EditorState;
use crate::theme::{self, Ink, T11M_LH, T12, T12M};

// Row/header glyphs `theme` doesn't carry (lucide stroke bodies, same
// 24-viewBox `currentColor` convention as `theme.rs`'s `stroke_icon!`).
static MINUS: Icon = Icon {
    name: "minus",
    view: 24.0,
    body: "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M5 12h14\"/></g>",
};
static TRASH_2: Icon = Icon {
    name: "trash-2",
    view: 24.0,
    body: "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M3 6h18\"/><path d=\"M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6\"/><path d=\"M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2\"/><line x1=\"10\" x2=\"10\" y1=\"11\" y2=\"17\"/><line x1=\"14\" x2=\"14\" y1=\"11\" y2=\"17\"/></g>",
};
static REFRESH_CW: Icon = Icon {
    name: "refresh-cw",
    view: 24.0,
    body: "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8\"/><path d=\"M21 3v5h-5\"/><path d=\"M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16\"/><path d=\"M8 16H3v5\"/></g>",
};

/// Spec entry point (`workspace.rs` dispatches `changes::view(s)`): renders
/// only — every mutation happens in event handlers or through
/// [`ensure_selection`], which the reply worker calls with `&mut State`
/// when changes data lands (replies arrive outside the render pass).
pub fn view(s: &State) -> El {
    let ink = Ink::new();
    root_render(s, &ink, &s.editor.kit)
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// The four status groups in desktop order: (state key, diff layer name).
pub const GROUPS: [(&str, &str); 4] = [
    ("conflicts", "conflict"),
    ("staged", "staged"),
    ("unstaged", "unstaged"),
    ("untracked", "untracked"),
];

/// Entries of one group, in display order.
pub fn group_entries<'a>(
    data: &'a app_core::git_view::GitViewChanges,
    key: &str,
) -> &'a [app_core::git_view::StatusEntry] {
    match key {
        "conflicts" => &data.conflicts,
        "staged" => &data.staged,
        "unstaged" => &data.unstaged,
        "untracked" => &data.untracked,
        _ => &[],
    }
}

/// Display name: `orig → path` for renames/copies, else `path`.
pub fn entry_name(entry: &app_core::git_view::StatusEntry) -> String {
    match &entry.orig_path {
        Some(orig) => format!("{orig} → {}", entry.path),
        None => entry.path.clone(),
    }
}

/// Short status badge text (desktop `statusLabel`).
pub fn status_label(status: &str) -> &'static str {
    match status {
        "modified" => "Modified",
        "added" => "Added",
        "deleted" => "Deleted",
        "renamed" => "Renamed",
        "copied" => "Copied",
        "typechange" => "Typechange",
        "unmerged" => "Conflict",
        "untracked" => "Untracked",
        _ => "Changed",
    }
}

/// What a group-header action does, scoped exactly like the desktop:
/// staged → unstage everything (`paths: None`), unstaged/untracked → stage
/// every path in the group (unfiltered, like main's `groupEntries`),
/// conflicts → nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupAction {
    StageAll(Vec<String>),
    UnstageAll,
    None,
}

pub fn group_action(key: &str, entries: &[app_core::git_view::StatusEntry]) -> GroupAction {
    if entries.is_empty() {
        return GroupAction::None;
    }
    match key {
        "staged" => GroupAction::UnstageAll,
        "unstaged" | "untracked" => {
            GroupAction::StageAll(entries.iter().map(|e| e.path.clone()).collect())
        }
        _ => GroupAction::None,
    }
}

/// Per-row primary action plus whether discard is offered (main offers
/// discard only for unstaged/untracked rows, never conflicts or staged).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowKind {
    Stage,
    Unstage,
    None,
}

pub fn row_kind(key: &str) -> (RowKind, bool) {
    match key {
        "staged" => (RowKind::Unstage, false),
        "unstaged" | "untracked" => (RowKind::Stage, true),
        _ => (RowKind::None, false),
    }
}

/// Case-insensitive substring filter on the display name (desktop
/// `matchesQuery`); an empty/blank query matches everything.
pub fn matches_filter(name: &str, filter: &str) -> bool {
    let q = filter.trim().to_lowercase();
    q.is_empty() || name.to_lowercase().contains(&q)
}

/// Find the entry a `(layer, path)` selection points at.
pub fn find_entry<'a>(
    data: &'a app_core::git_view::GitViewChanges,
    layer: &str,
    path: &str,
) -> Option<&'a app_core::git_view::StatusEntry> {
    let key = if layer == "conflict" {
        "conflicts"
    } else {
        layer
    };
    group_entries(data, key).iter().find(|e| e.path == path)
}

// ---------------------------------------------------------------------------
// Selection + fetch (needs &mut EditorState; driven by the backend)
// ---------------------------------------------------------------------------

/// Backend entry point: the reply worker calls this with `&mut State` after
/// `apply_reply` when the Changes section is active.
pub fn ensure_selection(s: &mut State) {
    ensure_selection_inner(&mut s.editor);
}

/// Keep `changes.selected/layer/patch_path` pointing at a real entry and
/// fetch its diff — the native equivalent of the desktop's "select first
/// change" effect. Only enqueues when the selection actually changes, so a
/// valid user selection is never stolen.
fn ensure_selection_inner(ed: &mut EditorState) {
    let Some(data) = ed.changes.data.clone() else {
        return;
    };
    let ok = ed
        .changes
        .selected
        .as_deref()
        .zip(ed.changes.layer.as_deref())
        .is_some_and(|(p, layer)| find_entry(&data, layer, p).is_some());
    if ok {
        return;
    }
    for (key, layer) in GROUPS {
        if let Some(entry) = group_entries(&data, key).first() {
            select_entry(ed, layer, &entry.path);
            return;
        }
    }
    ed.changes.selected = None;
    ed.changes.layer = None;
    ed.changes.patch_path = None;
    ed.changes.patch = None;
}

/// Point the patch pane at `(layer, path)` and enqueue its diff.
pub fn select_entry(ed: &mut EditorState, layer: &str, path: &str) {
    if ed.changes.layer.as_deref() == Some(layer) && ed.changes.patch_path.as_deref() == Some(path)
    {
        return;
    }
    ed.changes.selected = Some(path.to_string());
    ed.changes.layer = Some(layer.to_string());
    ed.changes.patch_path = Some(path.to_string());
    ed.changes.patch = None;
    ed.changes.patch_loading = false;
    // Drop the previous file's parsed diff so the pane can't flash stale
    // content while the new patch is in flight.
    ed.changes.diff.set_files(Vec::new());
    if let (Some(checkout), Some(key)) = (ed.checkout.clone(), ed.checkout_key.clone()) {
        // `enqueue_read`'s contract: the loading flag stays set only when the
        // job was accepted — a refused queue must not leave "Loading diff…".
        ed.changes.patch_loading = ed.enqueue_read(
            checkout,
            key,
            Req::Diff {
                layer: layer.to_string(),
                path: Some(path.to_string()),
                commit: None,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Shared stage/unstage/discard enqueue: the write rides the serialized
/// lane and `busy` holds until its reply lands. `busy` is armed only when
/// the queue accepted the job — a refusal returns `false` with the section
/// error + notice surfaced like a failed write, so no flag can stick.
pub fn write_paths(ed: &mut EditorState, kind: &'static str, req: Req) -> bool {
    let Some(checkout) = ed.checkout.clone() else {
        return false;
    };
    let Some(key) = ed.checkout_key.clone() else {
        return false;
    };
    if !ed.enqueue_write(checkout, key, req) {
        queue_full(ed);
        return false;
    }
    ed.changes.busy = Some(kind);
    ed.changes.error = None;
    true
}

/// Queue-refusal surfacing, mirroring `apply.rs`'s write-error arm: the
/// section error row plus the global notice.
fn queue_full(ed: &mut EditorState) {
    let text = "Backend queue is full — try again".to_string();
    ed.changes.error = Some(text.clone());
    ed.notice = Some(crate::editor::state::Notice { text, stale: false });
}

pub fn stage_paths(ed: &mut EditorState, paths: Option<Vec<String>>) {
    write_paths(ed, "stage", Req::Stage { paths });
}

pub fn unstage_paths(ed: &mut EditorState, paths: Option<Vec<String>>) {
    write_paths(ed, "unstage", Req::Unstage { paths });
}

/// Confirm the pending discard: one named file at its recorded layer. The
/// prompt survives a refused enqueue so the dialog stays open for a retry.
pub fn confirm_discard(ed: &mut EditorState) {
    let Some((layer, path)) = ed.changes.discard_prompt.clone() else {
        return;
    };
    if write_paths(ed, "discard", Req::Discard { layer, path }) {
        ed.changes.discard_prompt = None;
    }
}

pub fn cancel_discard(ed: &mut EditorState) {
    ed.changes.discard_prompt = None;
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn root_render(s: &State, ink: &Ink, k: &Kit) -> El {
    let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0);
    root = root.child(render(s, ink, k).grow(1.0).min_h(0.0));
    if s.editor.changes.discard_prompt.is_some() {
        root = root.child(discard_dialog(s, ink));
    }
    root
}

fn render(s: &State, ink: &Ink, k: &Kit) -> El {
    let changes = &s.editor.changes;
    match &changes.data {
        None if changes.loading => centered(ink, "Loading changes…"),
        None => {
            let mut pane = centered(ink, "");
            if let Some(err) = &changes.error {
                pane = pane.child(theme::txt(err, T12, ink.danger));
            } else {
                pane = pane.child(theme::txt("No changes data", T12, ink.muted));
            }
            pane.child(theme::btn_sm("Retry", None, true, ink).on_press_tx(
                |st: &mut State, _tx: &mut Tx| {
                    st.editor.changes.error = None;
                    st.editor.changes.loading = false;
                    if let (Some(c), Some(key)) =
                        (st.editor.checkout.clone(), st.editor.checkout_key.clone())
                    {
                        st.editor.changes.loading = st.editor.enqueue_read(c, key, Req::Changes);
                    }
                },
            ))
        }
        Some(data) => {
            let clean = GROUPS
                .iter()
                .all(|(key, _)| group_entries(data, key).is_empty());
            if clean {
                clean_view(s, ink)
            } else {
                split_view(s, ink, k, data)
            }
        }
    }
}

fn centered(ink: &Ink, label: &str) -> El {
    let mut pane = col()
        .grow(1.0)
        .w_full()
        .items_center()
        .justify_center()
        .gap(8.0);
    if !label.is_empty() {
        pane = pane.child(theme::txt(label, T12, ink.muted));
    }
    pane
}

/// Clean tree: desktop keeps Push reachable, so the commit panel stays.
fn clean_view(s: &State, ink: &Ink) -> El {
    col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(
            col()
                .grow(1.0)
                .w_full()
                .items_center()
                .justify_center()
                .gap(8.0)
                .child(theme::txt("✓", T12M, ink.success))
                .child(theme::txt("Working tree clean", T12M, ink.text))
                .child(theme::txt(
                    "No staged, unstaged, or untracked changes.",
                    T12,
                    ink.muted,
                ))
                .child(
                    theme::btn_sm("Refresh changes", None, true, ink).on_press_tx(
                        |st: &mut State, _tx: &mut Tx| {
                            st.editor.refresh_active();
                        },
                    ),
                ),
        )
        .child(crate::editor::commit::view(s))
}

fn split_view(s: &State, ink: &Ink, k: &Kit, data: &app_core::git_view::GitViewChanges) -> El {
    row()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .on_layout_tx(pane_layout::watch_row_width())
        .child(
            aside(s, ink, k, data)
                // Persisted width (desktop `editorList` default 288, min
                // 180, max 480 — per-section default keeps 300 for the
                // icon-row actions); the grip inside the aside drags it.
                .w(pane_layout::current(s, PaneKind::ChangesList))
                .h_full()
                .shrink()
                .min_w(pane_layout::PANE_MIN)
                .border_sides([0.0, 1.0, 0.0, 0.0], ink.border),
        )
        .child(
            patch_side(s, ink)
                .grow(1.0)
                .min_w(pane_layout::SIBLING_MIN)
                .h_full(),
        )
}

/// Left column: "Changed files" header + refresh, filter, scrollable groups,
/// commit panel pinned to the bottom (desktop `SplitPane` aside).
fn aside(s: &State, ink: &Ink, k: &Kit, data: &app_core::git_view::GitViewChanges) -> El {
    let mut col_el = col().h_full().w_full().min_h(0.0);
    col_el = col_el.child(
        row()
            .w_full()
            .h(34.0)
            .px(10.0)
            .items_center()
            .justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(theme::txt("CHANGED FILES", T11M_LH, ink.muted))
            .child(
                IconButton::new(&REFRESH_CW)
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .label("Refresh changes")
                    .help("Refresh changes")
                    .on_press_tx(|st: &mut State, _tx: &mut Tx| {
                        st.editor.refresh_active();
                    })
                    .build(k),
            ),
    );
    let filter_edit = &s.editor.changes.filter;
    col_el = col_el.child(
        row()
            .w_full()
            .px(10.0)
            .py(6.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(
                rune_kit::TextInput::new(filter_edit)
                    .placeholder("Filter changed files")
                    // `None` fills the row — a fixed width would overflow the
                    // ~280 px aside at the 760 px viewport.
                    .width(None)
                    .on_edit(move |st: &mut State| -> &mut rune::edit::TextEdit {
                        &mut st.editor.changes.filter
                    })
                    .build(k),
            ),
    );
    if let Some(err) = &s.editor.changes.error {
        col_el = col_el.child(
            row()
                .w_full()
                .px(10.0)
                .py(4.0)
                .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
                .child(theme::txt(err, T11M_LH, ink.danger)),
        );
    }
    let filter = s.editor.changes.filter.text.clone();
    // No grow/min_h inside the scroll viewport: the column must size to its
    // rows or `scroll_overflow` measures zero and reveals can't scroll.
    let mut groups = col().w_full().px(6.0).py(6.0).gap(2.0);
    for (key, layer) in GROUPS {
        groups = groups.child(group_block(s, ink, k, data, key, layer, &filter));
    }
    col_el = col_el.child(
        col()
            .child(groups)
            .key(keyboard::nav_scroll_scope(
                crate::editor::state::Section::Changes,
            ))
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .grow(1.0)
            .min_h(0.0)
            .w_full(),
    );
    col_el
        .child(crate::editor::commit::view(s))
        .child(pane_layout::grip(
            s,
            ink,
            PaneKind::ChangesList,
            "Resize changed files and diff",
        ))
}

fn group_block(
    s: &State,
    ink: &Ink,
    k: &Kit,
    data: &app_core::git_view::GitViewChanges,
    key: &'static str,
    layer: &'static str,
    filter: &str,
) -> El {
    let all = group_entries(data, key);
    let entries: Vec<_> = all
        .iter()
        .filter(|e| matches_filter(&entry_name(e), filter))
        .collect();
    let label = match key {
        "conflicts" => "Conflicts",
        "staged" => "Staged",
        "unstaged" => "Unstaged",
        "untracked" => "Untracked",
        _ => key,
    };
    let collapsed = s.editor.changes.collapsed.contains(key);
    let mut block = col().w_full().gap(1.0);

    // Header row: the toggle (chevron + label + count) grows; the scoped
    // group action is a sibling button so it never toggles the group.
    let mut head = row()
        .w_full()
        .h(26.0)
        .px(2.0)
        .gap(2.0)
        .items_center()
        .child(group_toggle(key, label, entries.len(), collapsed, ink));
    match group_action(key, all) {
        GroupAction::StageAll(_) => {
            head = head.child(
                Button::new("Stage all")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .disabled(s.editor.changes.busy.is_some())
                    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                        if st.editor.changes.busy.is_some() {
                            return;
                        }
                        // Recompute from live state — never the render-time list.
                        let paths = st.editor.changes.data.as_ref().map(|d| {
                            group_entries(d, key)
                                .iter()
                                .map(|e| e.path.clone())
                                .collect::<Vec<_>>()
                        });
                        if let Some(paths) = paths {
                            stage_paths(&mut st.editor, Some(paths));
                        }
                    })
                    .build(k),
            );
        }
        GroupAction::UnstageAll => {
            head = head.child(
                Button::new("Unstage all")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .disabled(s.editor.changes.busy.is_some())
                    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                        if st.editor.changes.busy.is_some() {
                            return;
                        }
                        unstage_paths(&mut st.editor, None);
                    })
                    .build(k),
            );
        }
        GroupAction::None => {}
    }
    block = block.child(head);

    if !collapsed {
        if entries.is_empty() {
            block = block.child(theme::txt("None", T12, ink.muted).px(12.0).py(2.0));
        }
        for entry in entries {
            block = block.child(entry_row(s, ink, k, key, layer, entry));
        }
    }
    block
}

/// Chevron + label + count; the whole area toggles the group.
fn group_toggle(key: &'static str, label: &str, count: usize, collapsed: bool, ink: &Ink) -> El {
    el().key(format!("group-toggle:{key}"))
        .grow(1.0)
        .min_w(0.0)
        .h(24.0)
        .px(4.0)
        .gap(5.0)
        .items_center()
        .radius(5.0)
        .hover(ink.text.alpha(0.06))
        .role("button")
        .label(format!("{label} files, {count}"))
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(theme::ic(
            if collapsed {
                &theme::CHEVRON_RIGHT
            } else {
                &theme::CHEVRON_DOWN
            },
            11.0,
            ink.muted,
        ))
        .child(theme::txt(label, T12M, ink.text))
        .child(theme::count_badge(count, ink))
        .on_press(move |st: &mut State| {
            if st.editor.changes.collapsed.contains(key) {
                st.editor.changes.collapsed.remove(key);
            } else {
                st.editor.changes.collapsed.insert(key.to_string());
            }
        })
}

/// One changed file: select button (file icon + path + status badge) plus
/// sibling icon actions so a write never also selects the row. Mirrors the
/// desktop row: the path truncates, the badge and actions never do, and the
/// accessible name keeps the full `orig → path`.
fn entry_row(
    s: &State,
    ink: &Ink,
    k: &Kit,
    key: &'static str,
    layer: &'static str,
    entry: &app_core::git_view::StatusEntry,
) -> El {
    let path = entry.path.clone();
    let name = entry_name(entry);
    let selected = s.editor.changes.selected.as_deref() == Some(entry.path.as_str())
        && s.editor.changes.layer.as_deref() == Some(layer);
    let nav_key = keyboard::change_row_key(layer, &entry.path);
    let nav_cur = keyboard::nav_active(s, crate::editor::state::Section::Changes, &nav_key);
    let busy = s.editor.changes.busy.is_some();
    let (kind, discard_ok) = row_kind(key);
    // One owned copy per closure: select, row action and discard each move
    // their own `String` so no handler can observe another's path.
    let sel_path = path.clone();
    let act_path = path.clone();
    let dis_path = path.clone();
    let mut row_el = row()
        .w_full()
        .h(24.0)
        .px(2.0)
        .gap(2.0)
        .items_center()
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .h(22.0)
                .px(4.0)
                .gap(5.0)
                .items_center()
                .radius(5.0)
                .key(nav_key)
                .fill(if nav_cur {
                    ink.accent.alpha(0.14)
                } else if selected {
                    ink.text.alpha(0.10)
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(ink.text.alpha(0.06))
                .role("button")
                .label(&name)
                .focusable(true)
                .focus_ring(2.0, 1.0, ink.accent)
                .child(theme::ic(rune_kit::file_icon(&entry.path), 12.0, ink.muted).a11y_hidden())
                .child(
                    theme::txt(&name, T12, ink.text)
                        .min_w(0.0)
                        .grow(1.0)
                        .truncate(),
                )
                .child(
                    el().px(4.0)
                        .py(0.5)
                        .radius(4.0)
                        .border(1.0, ink.border)
                        .child(theme::txt(status_label(entry.status), T11M_LH, ink.muted)),
                )
                .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                    select_entry(&mut st.editor, layer, &sel_path);
                }),
        );
    let row_icon = |icon: &'static Icon, verb: &str, path: &str| {
        IconButton::new(icon)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("{verb} {path}"))
            .help(verb)
            .disabled(busy)
    };
    match kind {
        RowKind::Stage => {
            row_el = row_el.child(
                row_icon(&theme::PLUS, "Stage", &path)
                    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                        if busy || st.editor.changes.busy.is_some() {
                            return;
                        }
                        stage_paths(&mut st.editor, Some(vec![act_path.clone()]));
                    })
                    .build(k),
            );
        }
        RowKind::Unstage => {
            row_el = row_el.child(
                row_icon(&MINUS, "Unstage", &path)
                    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                        if busy || st.editor.changes.busy.is_some() {
                            return;
                        }
                        unstage_paths(&mut st.editor, Some(vec![act_path.clone()]));
                    })
                    .build(k),
            );
        }
        RowKind::None => {}
    }
    if discard_ok {
        let layer_owned = layer.to_string();
        row_el = row_el.child(
            row_icon(&TRASH_2, "Discard", &path)
                .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                    if busy || st.editor.changes.busy.is_some() {
                        return;
                    }
                    st.editor.changes.discard_prompt =
                        Some((layer_owned.clone(), dis_path.clone()));
                })
                .build(k),
        );
    }
    row_el
}

/// Right side: empty hint or the shared patch pane for Pane::Changes.
fn patch_side(s: &State, ink: &Ink) -> El {
    if s.editor.changes.selected.is_none() {
        return col()
            .grow(1.0)
            .items_center()
            .justify_center()
            .child(theme::txt(
                "Select a changed file to see its diff",
                T12,
                ink.muted,
            ));
    }
    patch::view(s, Pane::Changes)
}

/// Discard confirmation overlay (named file, cannot be undone) — same modal
/// pattern as the draft guard in `workspace.rs`.
fn discard_dialog(s: &State, ink: &Ink) -> El {
    let (layer, path) = s.editor.changes.discard_prompt.clone().unwrap_or_default();
    let name = s
        .editor
        .changes
        .data
        .as_ref()
        .and_then(|d| find_entry(d, &layer, &path))
        .map(entry_name)
        .unwrap_or(path);
    col()
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rune::Color::BLACK.alpha(0.45))
        .items_center()
        .justify_center()
        .child(
            col()
                .w(400.0)
                .gap(12.0)
                .padding(16.0)
                .radius(12.0)
                .fill(ink.panel)
                .border(1.0, ink.border)
                .child(theme::txt("Discard changes?", T12M, ink.text))
                .child(theme::txt(
                    &format!("Discard the changes to {name}? This cannot be undone."),
                    T12,
                    ink.muted,
                ))
                .child(
                    row()
                        .gap(8.0)
                        .justify_end()
                        .w_full()
                        .child(theme::btn_sm("Cancel", None, true, ink).on_press(
                            |st: &mut State| {
                                cancel_discard(&mut st.editor);
                            },
                        ))
                        .child(theme::btn_sm("Discard", None, false, ink).on_press(
                            |st: &mut State| {
                                confirm_discard(&mut st.editor);
                            },
                        )),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::git_view::StatusEntry;

    fn entry(path: &str, orig: Option<&str>, status: &'static str) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: orig.map(Into::into),
            status,
            added: None,
            removed: None,
        }
    }

    fn changes() -> app_core::git_view::GitViewChanges {
        app_core::git_view::GitViewChanges {
            branch: Some("main".into()),
            head: "abc".into(),
            conflicts: vec![],
            staged: vec![entry("src/a.rs", None, "modified")],
            unstaged: vec![entry("src/b.rs", Some("src/old.rs"), "renamed")],
            untracked: vec![entry("new.txt", None, "untracked")],
            ahead: Some(2),
        }
    }

    #[test]
    fn group_order_and_entries() {
        let d = changes();
        assert_eq!(group_entries(&d, "staged").len(), 1);
        assert_eq!(group_entries(&d, "unstaged")[0].path, "src/b.rs");
        assert!(group_entries(&d, "conflicts").is_empty());
        assert!(group_entries(&d, "nope").is_empty());
    }

    #[test]
    fn entry_name_shows_rename_source() {
        let e = entry("src/b.rs", Some("src/old.rs"), "renamed");
        assert_eq!(entry_name(&e), "src/old.rs → src/b.rs");
        assert_eq!(entry_name(&entry("x", None, "modified")), "x");
    }

    #[test]
    fn status_labels_cover_git_set() {
        assert_eq!(status_label("modified"), "Modified");
        assert_eq!(status_label("unmerged"), "Conflict");
        assert_eq!(status_label("future"), "Changed");
    }

    #[test]
    fn group_action_scopes_like_main() {
        let d = changes();
        // Staged group unstages ALL (paths None upstream).
        assert_eq!(
            group_action("staged", group_entries(&d, "staged")),
            GroupAction::UnstageAll
        );
        // Unstaged group stages exactly its own paths.
        match group_action("unstaged", group_entries(&d, "unstaged")) {
            GroupAction::StageAll(paths) => assert_eq!(paths, vec!["src/b.rs".to_string()]),
            other => panic!("expected StageAll, got {other:?}"),
        }
        // Conflicts offer no group write even when non-empty.
        let mut d2 = changes();
        d2.conflicts = vec![entry("c.rs", None, "unmerged")];
        assert_eq!(
            group_action("conflicts", group_entries(&d2, "conflicts")),
            GroupAction::None
        );
        // Empty groups have no action.
        assert_eq!(group_action("untracked", &[]), GroupAction::None);
    }

    #[test]
    fn row_kinds_match_main() {
        assert_eq!(row_kind("staged"), (RowKind::Unstage, false));
        assert_eq!(row_kind("unstaged"), (RowKind::Stage, true));
        assert_eq!(row_kind("untracked"), (RowKind::Stage, true));
        assert_eq!(row_kind("conflicts"), (RowKind::None, false));
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        assert!(matches_filter("src/B.rs", "b.rs"));
        assert!(matches_filter("src/b.rs", "  "));
        assert!(!matches_filter("src/b.rs", "zzz"));
    }

    #[test]
    fn find_entry_maps_conflict_layer() {
        let mut d = changes();
        d.conflicts = vec![entry("c.rs", None, "unmerged")];
        assert!(find_entry(&d, "conflict", "c.rs").is_some());
        assert!(find_entry(&d, "staged", "src/a.rs").is_some());
        assert!(find_entry(&d, "staged", "nope").is_none());
    }

    // -- headless runtime: real widget input + full-scene footer layout ---

    use rune::{Cx, Env, Key, KeyEvent, Mods, Runtime};
    use std::path::PathBuf;

    /// Restores `GOKO_HOME` on drop so the throwaway home never leaks into
    /// parallel tests (same guard the commit picker test uses).
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

    /// Enough changed rows to overflow the file list at both QA viewports.
    fn big_changes() -> app_core::git_view::GitViewChanges {
        let mut d = changes();
        d.staged = (0..30)
            .map(|i| entry(&format!("src/staged-{i:02}.rs"), None, "modified"))
            .collect();
        d.unstaged = (0..30)
            .map(|i| entry(&format!("src/unstaged-{i:02}.rs"), None, "modified"))
            .collect();
        d.untracked = (0..15)
            .map(|i| entry(&format!("misc/new-{i:02}.txt"), None, "untracked"))
            .collect();
        d.unstaged.push(entry("README.md", None, "modified"));
        d
    }

    /// The QA scene: editor view, one project + checkout, populated Changes,
    /// one disconnected ACP agent so the whole commit footer renders.
    fn editor_scene() -> (State, app_core::test_support::TempDir, HomeGuard) {
        let home = app_core::test_support::TempDir::new("changes-qa");
        let guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = crate::View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = crate::editor::state::Section::Changes;
        s.editor.acp.agents = vec![app_core::acp::AcpAgent {
            id: "a".into(),
            label: "Agent A".into(),
            enabled: true,
            command: "cmd".into(),
            prompt: String::new(),
            available: true,
            is_default: false,
        }];
        s.editor.acp.statuses.insert(
            "a".into(),
            app_core::acp::AcpStatus {
                state: app_core::acp::AgentState::Disconnected,
                message: None,
            },
        );
        s.editor.changes.data = Some(big_changes());
        s.editor.changes.loading = false;
        (s, home, guard)
    }

    fn changes_only(s: &State, _cx: &mut Cx<State>) -> El {
        view(s)
    }

    /// The changed-files/diff split exposes the same draggable separator as
    /// the other panes; dragging persists `changesList`.
    #[test]
    fn pane_grip_drags_changes_list() {
        let (s, _home, _guard) = editor_scene();
        let mut rt = Runtime::new(s, changes_only, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let g = rt
            .elements()
            .into_iter()
            .find(|e| {
                e.role == "separator" && e.label.as_deref() == Some("Resize changed files and diff")
            })
            .expect("changes pane grip mounted");
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        for i in 1..=4 {
            rt.pointer_move(x + 60.0 * i as f32 / 4.0, y);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x + 60.0, y);
        rt.frame(1.0 / 60.0);

        let w = rt.state.editor.history.layout.panes.changes_list;
        assert!((w - 360.0).abs() < 1.0, "drag +60px: 300 -> {w}");
        assert!(rt.state.editor.history.layout_dirty);
    }

    fn click(rt: &mut Runtime<State>, x: f32, y: f32) {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn type_text(rt: &mut Runtime<State>, text: &str) {
        for ch in text.chars() {
            rt.key(KeyEvent {
                key: Key::Text(ch.to_string()),
                mods: Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
    }

    fn backspace(rt: &mut Runtime<State>, n: usize) {
        for _ in 0..n {
            rt.key(KeyEvent {
                key: Key::Backspace,
                mods: Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
    }

    fn labels(rt: &Runtime<State>) -> Vec<String> {
        rt.elements().into_iter().filter_map(|e| e.label).collect()
    }

    /// CUA repro for the dead filter: click the field, type `README`, and
    /// the parent `TextEdit` + visible rows must follow; clearing restores.
    #[test]
    fn filter_input_types_and_filters_rows() {
        let (s, _home, _guard) = editor_scene();
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // The filter is the topmost textbox (the commit draft sits lower).
        let tb = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "textbox")
            .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
            .expect("filter textbox mounted");
        let r = tb.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);

        type_text(&mut rt, "README");
        assert_eq!(rt.state.editor.changes.filter.text, "README");
        let ls = labels(&rt);
        assert!(ls.iter().any(|l| l == "README.md"), "match stays: {ls:?}");
        assert!(
            !ls.iter().any(|l| l == "src/staged-00.rs"),
            "non-match hides: {ls:?}"
        );

        backspace(&mut rt, 6);
        assert_eq!(rt.state.editor.changes.filter.text, "");
        let ls = labels(&rt);
        assert!(
            ls.iter().any(|l| l == "src/staged-00.rs"),
            "cleared filter restores rows: {ls:?}"
        );
    }

    /// CUA repro for the buried footer: in the FULL app view the commit
    /// actions must sit inside the viewport at both QA sizes while the
    /// changed-file rows are the part that scrolls off.
    #[test]
    fn commit_footer_stays_visible_at_both_viewports() {
        for (w, h) in [(1512.0, 814.0), (760.0, 480.0)] {
            let (s, _home, _guard) = editor_scene();
            let mut rt = Runtime::new(s, crate::view, (w, h), Env::new(true, 2.0));
            rt.frame(0.0);

            for prefix in ["Connect", "Generate", "Commit", "Push"] {
                let e = rt
                    .elements()
                    .into_iter()
                    .find(|e| {
                        e.role == "button"
                            && e.label.as_deref().is_some_and(|l| l.starts_with(prefix))
                    })
                    .unwrap_or_else(|| panic!("{prefix} button missing at {w}x{h}"));
                let r = e.rect;
                assert!(
                    r[1] >= -0.5 && r[1] + r[3] <= h + 0.5,
                    "{prefix} rect {r:?} escapes {w}x{h} viewport"
                );
                assert!(
                    r[0] >= -0.5 && r[0] + r[2] <= w + 0.5,
                    "{prefix} rect {r:?} overflows {w}px width"
                );
            }

            // The file list is what overflows: rows continue below the fold.
            let below = rt.elements().iter().any(|e| {
                e.role == "button"
                    && e.label.as_deref().is_some_and(|l| {
                        l.starts_with("src/") || l.starts_with("misc/") || l == "README.md"
                    })
                    && e.rect[1] + e.rect[3] > h
            });
            assert!(below, "file rows should extend below the {h}px fold");
        }
    }

    /// Layout-seam proof for the footer overflow (parent-owned fix): the
    /// editor content reaches the window through `rune_kit::root`, whose
    /// wrapper is `el().w_full().h_full()` with NO grow. As a natural-height
    /// flex child its `h_full` resolves to the column's FULL height, ignoring
    /// the tab/controls bars above it — the whole workspace lands ~85 px low.
    /// `terminal_area` avoids this by putting `.grow(1).min_h(0)` on its own
    /// root; the kit-root wrapper needs the same. This test reproduces both
    /// sides of that seam without touching parent files.
    #[test]
    fn kit_root_wrapper_needs_grow_to_fill_remaining_space() {
        fn scene(grow: bool, s: &State) -> El {
            let ink = &s.ink;
            let base = col().w_full().h_full().min_h(0.0).child(theme::txt(
                "MARKER",
                theme::T12,
                ink.text,
            ));
            let content = rune_kit::root(base, Vec::new());
            let content = if grow {
                content.grow(1.0).min_h(0.0)
            } else {
                content
            };
            // A genuinely bounded 200 px column (the view root always fills
            // the viewport, so the fixed height must live one level down —
            // like the real middle column bounded by the window row).
            col().w_full().h_full().child(
                col()
                    .w_full()
                    .h(200.0)
                    .child(el().h(40.0))
                    .child(el().h(40.0))
                    .child(content),
            )
        }
        fn plain(s: &State, _cx: &mut Cx<State>) -> El {
            scene(false, s)
        }
        fn grown(s: &State, _cx: &mut Cx<State>) -> El {
            scene(true, s)
        }

        let home = app_core::test_support::TempDir::new("kit-root-grow");
        let _guard = HomeGuard::set(home.path());

        // Without grow: the h_full wrapper claims the full 200 px column
        // (bottom 280, below the fold — like the QA footer). With
        // grow(1).min_h(0): it claims only the 120 px leftover.
        let kit_root = |rt: &Runtime<State>| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("kit-root"))
                .expect("kit-root mounted")
                .rect
        };

        let mut rt = Runtime::new(
            State::new(crate::test_handle()),
            plain,
            (400.0, 300.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        let r = kit_root(&rt);
        assert!(
            r[1] + r[3] > 200.0,
            "un-grew kit-root should overflow the column: {r:?}"
        );

        let mut rt = Runtime::new(
            State::new(crate::test_handle()),
            grown,
            (400.0, 300.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        let r = kit_root(&rt);
        assert!(
            r[1] + r[3] <= 200.0,
            "grew kit-root should fit the leftover: {r:?}"
        );
    }

    // -- refused-queue regressions: flags must never stick ---------------
    //
    // `project_id = None` makes `enqueue_write`/`enqueue_read` return `false`
    // deterministically — the same contract a full or disconnected queue
    // gives. No reply will ever arrive, so every flag the caller armed must
    // be settled by the caller itself.

    #[test]
    fn rejected_writes_never_arm_busy() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.project_id = None;

        stage_paths(&mut s.editor, Some(vec!["a".into()]));
        assert_eq!(s.editor.changes.busy, None, "stage refusal: busy clear");
        unstage_paths(&mut s.editor, None);
        assert_eq!(s.editor.changes.busy, None, "unstage refusal: busy clear");

        s.editor.changes.discard_prompt = Some(("unstaged".into(), "b.txt".into()));
        confirm_discard(&mut s.editor);
        assert_eq!(s.editor.changes.busy, None, "discard refusal: busy clear");
        assert!(
            s.editor.changes.discard_prompt.is_some(),
            "rejected discard keeps the confirmation open for a retry"
        );
        assert!(
            s.editor.changes.error.is_some(),
            "refusal surfaces an error"
        );
        assert!(s.editor.notice.is_some(), "refusal raises a notice");
    }

    #[test]
    fn accepted_write_arms_busy() {
        let (mut s, _home, _guard) = editor_scene();
        stage_paths(&mut s.editor, Some(vec!["a".into()]));
        assert_eq!(s.editor.changes.busy, Some("stage"));
        assert!(s.editor.changes.error.is_none());
    }

    #[test]
    fn accepted_discard_clears_prompt() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.changes.discard_prompt = Some(("unstaged".into(), "b.txt".into()));
        confirm_discard(&mut s.editor);
        assert!(s.editor.changes.discard_prompt.is_none());
        assert_eq!(s.editor.changes.busy, Some("discard"));
    }

    #[test]
    fn rejected_diff_read_leaves_patch_loading_clear() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.project_id = None;
        select_entry(&mut s.editor, "unstaged", "src/b.rs");
        assert!(!s.editor.changes.patch_loading);
        // The selection itself still lands — only the fetch was refused.
        assert_eq!(s.editor.changes.patch_path.as_deref(), Some("src/b.rs"));
    }
}

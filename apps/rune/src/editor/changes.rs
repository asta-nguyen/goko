//! Changes section: the checkout's staged/unstaged/untracked/conflict groups
//! with per-row and per-group Git writes, the discard confirmation, the
//! changed-file filter, and the shared patch pane (`patch::view`).
//!
//! Mirrors the desktop `EditorChanges.tsx`: groups keep main's order
//! (conflicts → staged → unstaged → untracked); conflict rows open the editor,
//! other rows select a diff (action buttons are siblings of the select row),
//! "Stage all" scopes to that group's paths while "Unstage all" sends
//! `paths: None`, and discard is a named one-file confirmation.

use rune::{El, Icon, Tx, col, el, row};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Menu, MenuItem,
    ScrollBar, context_menu,
};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::history_layout::PaneKind;
use crate::editor::keyboard;
use crate::editor::pane_layout;
use crate::editor::patch::{self, Pane};
use crate::editor::state::{ChangeGroup, EditorState, ListMenu, ListView};
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
static LIST_TREE: Icon = Icon {
    name: "list-tree",
    view: 24.0,
    body: "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M21 12h-8\"/><path d=\"M21 6H8\"/><path d=\"M21 18h-8\"/><path d=\"M3 6v4c0 1.1.9 2 2 2h3\"/><path d=\"M3 10v6c0 1.1.9 2 2 2h3\"/></g>",
};

/// Key of the header's view button; the anchored menu opens beside it.
const VIEW_MENU_KEY: &str = "changes-view-menu";

/// Spec entry point (`workspace.rs` dispatches `changes::view(s)`): renders
/// only — every mutation happens in event handlers or through
/// [`ensure_selection`], which the reply worker calls with `&mut State`
/// when changes data lands (replies arrive outside the render pass).
pub fn view(s: &State) -> El {
    let ink = Ink::new();
    let mut layer = Vec::new();
    let mut root = root_render(s, &ink, &s.editor.kit);
    if let Some(menu) = list_menu(s, &mut layer) {
        root = root.child(menu);
    }
    rune_kit::root(root, layer)
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

/// Maximum number of rows rendered per group in the changed files list before
/// truncating with an informative note. Prevents unbounded UI allocation and
/// stutter when repositories have thousands of untracked or changed files.
pub const MAX_GROUP_RENDER: usize = 200;

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

/// Display label for a rendered group header (canonical key or `tracked`).
pub fn group_label(key: &str) -> &'static str {
    match key {
        "conflicts" => "Conflicts",
        "staged" => "Staged",
        "unstaged" => "Unstaged",
        "untracked" => "Untracked",
        "tracked" => "Tracked",
        _ => "Changed",
    }
}

/// One group the list renders for the active `Group By` mode.
pub(crate) struct RowGroup<'a> {
    /// Collapse/identity key: a canonical group key or `tracked`.
    pub key: &'static str,
    pub label: &'static str,
    /// `(entry, canonical group key, diff layer)` in display order — the
    /// composite modes re-tag each entry with its own key/layer so row
    /// actions, selection, and writes stay per-layer.
    pub entries: Vec<(
        &'a app_core::git_view::StatusEntry,
        &'static str,
        &'static str,
    )>,
}

/// The groups the list renders for `mode`, in display order. `None` is the
/// canonical four (today's order); the composite modes emit no data-empty
/// group.
pub(crate) fn list_groups(
    data: &app_core::git_view::GitViewChanges,
    mode: ChangeGroup,
) -> Vec<RowGroup<'_>> {
    fn tagged<'a>(
        entries: &'a [app_core::git_view::StatusEntry],
        key: &'static str,
        layer: &'static str,
    ) -> Vec<(
        &'a app_core::git_view::StatusEntry,
        &'static str,
        &'static str,
    )> {
        entries.iter().map(|e| (e, key, layer)).collect()
    }
    match mode {
        ChangeGroup::None => GROUPS
            .iter()
            .map(|(key, layer)| RowGroup {
                key,
                label: group_label(key),
                entries: tagged(group_entries(data, key), key, layer),
            })
            .collect(),
        ChangeGroup::TrackedUntracked => [
            RowGroup {
                key: "tracked",
                label: "Tracked",
                entries: [
                    tagged(&data.staged, "staged", "staged"),
                    tagged(&data.unstaged, "unstaged", "unstaged"),
                    tagged(&data.conflicts, "conflicts", "conflict"),
                ]
                .concat(),
            },
            RowGroup {
                key: "untracked",
                label: "Untracked",
                entries: tagged(&data.untracked, "untracked", "untracked"),
            },
        ]
        .into_iter()
        .filter(|g| !g.entries.is_empty())
        .collect(),
        ChangeGroup::StagedUnstaged => [
            RowGroup {
                key: "staged",
                label: "Staged",
                entries: [
                    tagged(&data.staged, "staged", "staged"),
                    tagged(&data.conflicts, "conflicts", "conflict"),
                ]
                .concat(),
            },
            RowGroup {
                key: "unstaged",
                label: "Unstaged",
                entries: [
                    tagged(&data.unstaged, "unstaged", "unstaged"),
                    tagged(&data.untracked, "untracked", "untracked"),
                ]
                .concat(),
            },
        ]
        .into_iter()
        .filter(|g| !g.entries.is_empty())
        .collect(),
    }
}

/// Collapse key for a directory row in the Changes `Tree` view — the `dir:`
/// prefix keeps it distinct from the group keys in `changes.collapsed`.
pub(crate) fn dir_key(path: &str) -> String {
    format!("dir:{path}")
}

/// Extra left padding per nesting level of a `Tree` row.
pub(crate) const DIR_INDENT: f32 = 14.0;

/// One row of a `Tree`-view changed-file list: a directory header, or a
/// file nested `depth` directories deep carrying its caller's data.
pub(crate) enum TreeRow<'a, T> {
    /// `path` is the full directory path (its collapse key), `name` the
    /// last segment.
    Dir {
        path: String,
        name: String,
        depth: usize,
    },
    File {
        entry: &'a app_core::git_view::StatusEntry,
        data: T,
        depth: usize,
    },
}

struct DirNode<'a, T> {
    dirs: Vec<(String, DirNode<'a, T>)>,
    files: Vec<(&'a app_core::git_view::StatusEntry, T)>,
}

fn tree_insert<'a, T: Copy>(
    node: &mut DirNode<'a, T>,
    segments: &[&str],
    entry: &'a app_core::git_view::StatusEntry,
    data: T,
) {
    let mut cur = node;
    for seg in segments {
        let pos = match cur.dirs.iter().position(|(n, _)| n == seg) {
            Some(i) => i,
            None => {
                cur.dirs.push((
                    (*seg).to_string(),
                    DirNode {
                        dirs: Vec::new(),
                        files: Vec::new(),
                    },
                ));
                cur.dirs.len() - 1
            }
        };
        cur = &mut cur.dirs[pos].1;
    }
    cur.files.push((entry, data));
}

fn tree_emit<'n, 'a, T: Copy>(
    node: &'n DirNode<'a, T>,
    prefix: &str,
    depth: usize,
    is_dir_collapsed: &impl Fn(&str) -> bool,
    out: &mut Vec<TreeRow<'a, T>>,
) {
    let mut dirs: Vec<&(String, DirNode<'a, T>)> = node.dirs.iter().collect();
    dirs.sort_by(|a, b| rune_kit::natural_cmp(&a.0, &b.0));
    let mut files = node.files.clone();
    files.sort_by(|a, b| {
        rune_kit::natural_cmp(
            a.0.path.rsplit('/').next().unwrap_or(&a.0.path),
            b.0.path.rsplit('/').next().unwrap_or(&b.0.path),
        )
    });
    for (name, child) in dirs {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        out.push(TreeRow::Dir {
            path: path.clone(),
            name: name.clone(),
            depth,
        });
        if !is_dir_collapsed(&path) {
            tree_emit(child, &path, depth + 1, is_dir_collapsed, out);
        }
    }
    for (entry, data) in files {
        out.push(TreeRow::File { entry, data, depth });
    }
}

/// Nest `entries` under directories derived from `entry.path`: each level
/// emits its subdirectories first (`natural_cmp`), then its files. A
/// collapsed directory emits only its own row until expanded again —
/// `is_dir_collapsed` receives the full dir path and the caller keys its
/// own collapse set.
pub(crate) fn tree_rows<'a, T: Copy>(
    entries: &[(&'a app_core::git_view::StatusEntry, T)],
    is_dir_collapsed: impl Fn(&str) -> bool,
) -> Vec<TreeRow<'a, T>> {
    let mut root = DirNode {
        dirs: Vec::new(),
        files: Vec::new(),
    };
    for &(entry, data) in entries {
        let mut segs: Vec<&str> = entry.path.split('/').collect();
        segs.pop(); // the last segment is the file name, not a directory
        tree_insert(&mut root, &segs, entry, data);
    }
    let mut out = Vec::new();
    tree_emit(&root, "", 0, &is_dir_collapsed, &mut out);
    out
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

pub fn group_action<'a>(
    key: &str,
    entries: impl IntoIterator<Item = &'a app_core::git_view::StatusEntry>,
) -> GroupAction {
    let paths: Vec<String> = entries.into_iter().map(|e| e.path.clone()).collect();
    if paths.is_empty() {
        return GroupAction::None;
    }
    match key {
        "staged" => GroupAction::UnstageAll,
        "unstaged" | "untracked" => GroupAction::StageAll(paths),
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

/// Shared pointer/keyboard activation keeps conflict files on the guarded edit path.
pub fn activate_entry(s: &mut State, layer: &str, path: &str, tx: &mut Tx) {
    if layer == "conflict" {
        crate::editor::files::open_doc(s, path, None, tx);
    } else {
        select_entry(&mut s.editor, layer, path);
    }
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

/// Clean tree: desktop keeps Push reachable, so the commit panel stays. The
/// list menu still opens on a secondary press — the empty list is the one
/// place the `View`/`Group By` choice survives a clean worktree.
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
                .on_secondary_press(|st: &mut State, p: rune::PointerPress, _tx: &mut Tx| {
                    st.editor.changes.menu = Some(ListMenu::Point { x: p.x, y: p.y });
                })
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
                row()
                    .gap(2.0)
                    .items_center()
                    .child(
                        IconButton::new(&LIST_TREE)
                            .variant(IconButtonVariant::GhostMuted)
                            .control_size(ControlSize::Small)
                            .label("View and group changed files")
                            .help("View and group changed files")
                            .on_press(|st: &mut State| {
                                st.editor.changes.menu = Some(ListMenu::Anchor);
                            })
                            .build(k)
                            .key(VIEW_MENU_KEY),
                    )
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
    for g in list_groups(data, s.editor.changes.group) {
        groups = groups.child(group_block(s, ink, k, &g, &filter));
    }
    col_el = col_el.child(
        col()
            .child(groups)
            .key(keyboard::nav_scroll_scope(
                crate::editor::state::Section::Changes,
            ))
            // The list's View/Group-By menu opens on a secondary press
            // anywhere over the rows — it is list-level, not row-level.
            .on_secondary_press(|st: &mut State, p: rune::PointerPress, _tx: &mut Tx| {
                st.editor.changes.menu = Some(ListMenu::Point { x: p.x, y: p.y });
            })
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .scroll_bar(k),
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

fn group_block(s: &State, ink: &Ink, k: &Kit, group: &RowGroup<'_>, filter: &str) -> El {
    let entries: Vec<(&app_core::git_view::StatusEntry, &'static str, &'static str)> = group
        .entries
        .iter()
        .filter(|(e, _, _)| matches_filter(&entry_name(e), filter))
        .copied()
        .collect();
    let collapsed = s.editor.changes.collapsed.contains(group.key);
    let mut block = col().w_full().gap(2.0);

    // Header row: the toggle (chevron + label + count) grows; the scoped
    // group action is a sibling button so it never toggles the group.
    let mut head = row()
        .w_full()
        .h(26.0)
        .px(2.0)
        .gap(2.0)
        .items_center()
        .child(group_toggle(
            group.key,
            group.label,
            entries.len(),
            collapsed,
            ink,
        ));
    match group_action(group.key, entries.iter().map(|(e, _, _)| *e)) {
        GroupAction::StageAll(_) => {
            let key = group.key;
            head = head.child(
                Button::new("Stage all")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .disabled(s.editor.changes.busy.is_some())
                    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
                        if st.editor.changes.busy.is_some() {
                            return;
                        }
                        // Recompute from live state — never the render-time
                        // list; composite groups hold more than one layer.
                        let paths = st.editor.changes.data.as_ref().map(|d| {
                            list_groups(d, st.editor.changes.group)
                                .into_iter()
                                .find(|g| g.key == key)
                                .map(|g| {
                                    g.entries
                                        .iter()
                                        .map(|(e, _, _)| e.path.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
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

    if collapsed {
        return block;
    }
    if entries.is_empty() {
        return block.child(theme::txt("None", T12, ink.muted).px(12.0).py(2.0));
    }

    let total = entries.len();
    // Past the render cap the selected and j/k-target rows still mount —
    // appended flat in `List`, folded into the tree at their natural spot
    // in `Tree`.
    let mut render: Vec<(&app_core::git_view::StatusEntry, &'static str, &'static str)> =
        entries.iter().take(MAX_GROUP_RENDER).copied().collect();
    let mut extras: Vec<(&app_core::git_view::StatusEntry, &'static str, &'static str)> =
        Vec::new();
    if total > MAX_GROUP_RENDER {
        if let (Some(sel), Some(lay)) = (
            s.editor.changes.selected.as_deref(),
            s.editor.changes.layer.as_deref(),
        ) && let Some(pos) = entries
            .iter()
            .position(|(e, _, l)| e.path == sel && *l == lay)
            && pos >= MAX_GROUP_RENDER
        {
            extras.push(entries[pos]);
        }
        if let Some(nav) = &s.editor.keyboard.nav
            && nav.section == crate::editor::state::Section::Changes
            && let Some(rest) = nav.key.strip_prefix("goko-nav-change:")
            && let Some((nav_layer, nav_path)) = rest.split_once(':')
            && let Some(pos) = entries
                .iter()
                .position(|(e, _, l)| e.path == nav_path && *l == nav_layer)
            && pos >= MAX_GROUP_RENDER
            && !extras.contains(&entries[pos])
        {
            extras.push(entries[pos]);
        }
    }

    match s.editor.list_view {
        ListView::List => {
            for &(entry, key, layer) in render.iter().chain(extras.iter()) {
                block = block.child(entry_row(s, ink, k, key, layer, entry, 0.0));
            }
        }
        ListView::Tree => {
            render.extend(extras);
            let flat: Vec<(
                &app_core::git_view::StatusEntry,
                (&'static str, &'static str),
            )> = render.iter().map(|&(e, k, l)| (e, (k, l))).collect();
            let rows = tree_rows(&flat, |p| s.editor.changes.collapsed.contains(&dir_key(p)));
            for trow in rows {
                block = block.child(match trow {
                    TreeRow::Dir { path, name, depth } => {
                        dir_row(s, ink, group.key, &path, &name, depth)
                    }
                    TreeRow::File {
                        entry,
                        data: (key, layer),
                        depth,
                    } => entry_row(s, ink, k, key, layer, entry, depth as f32 * DIR_INDENT),
                });
            }
        }
    }

    if total > MAX_GROUP_RENDER {
        block = block.child(
            row()
                .w_full()
                .px(10.0)
                .py(4.0)
                .items_center()
                .child(theme::txt(
                    &format!(
                        "+{} more changed files (use filter to narrow)",
                        total - MAX_GROUP_RENDER
                    ),
                    T11M_LH,
                    ink.muted,
                )),
        );
    }
    block
}

/// A directory header row in `Tree` view: chevron + folder icon + the dir's
/// last segment. The whole row toggles `dir:{path}` in `changes.collapsed` —
/// the same collapse model as the group headers, namespaced by `dir:`.
fn dir_row(
    s: &State,
    ink: &Ink,
    group_key: &'static str,
    path: &str,
    name: &str,
    depth: usize,
) -> El {
    let key = dir_key(path);
    let collapsed = s.editor.changes.collapsed.contains(&key);
    el().key(format!("change-dir:{group_key}:{path}"))
        .w_full()
        .h(24.0)
        .px(2.0)
        .pl(4.0 + depth as f32 * DIR_INDENT)
        .gap(4.0)
        .items_center()
        .radius(6.0)
        .hover(ink.text.alpha(0.06))
        .role("button")
        .label(format!("{path} directory"))
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
        .child(theme::ic(&theme::FOLDER, 12.0, ink.muted))
        .child(
            theme::txt(name, T12M, ink.text)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(path.to_string()),
        )
        .on_press(move |st: &mut State| {
            if !st.editor.changes.collapsed.remove(&key) {
                st.editor.changes.collapsed.insert(key.clone());
            }
        })
}

/// The changed-file list's context menu (`View` + `Group By`), mounted as a
/// kit layer while `changes.menu` is open — at the secondary-press point, or
/// beside the header's view button when that opened it.
fn list_menu(s: &State, layer: &mut Vec<El>) -> Option<El> {
    let m = s.editor.changes.menu.as_ref()?;
    let menu = match m {
        ListMenu::Point { x, y } => context_menu(
            "changes-list",
            el(),
            Some(rune::PointerPress { x: *x, y: *y }),
        ),
        ListMenu::Anchor => Menu::new("changes-list").anchor(VIEW_MENU_KEY).open(true),
    };
    let view = s.editor.list_view;
    let group = s.editor.changes.group;
    let view_item = |label: &'static str, v: ListView| {
        MenuItem::new(label)
            .checked(view == v)
            .on_select(move |st: &mut State| {
                st.editor.list_view = v;
                st.editor.changes.menu = None;
            })
    };
    let group_item = |label: &'static str, g: ChangeGroup| {
        MenuItem::new(label)
            .checked(group == g)
            .on_select(move |st: &mut State| {
                st.editor.changes.group = g;
                st.editor.changes.menu = None;
            })
    };
    Some(
        menu.group_label("View")
            .item(view_item("List", ListView::List))
            .item(view_item("Tree", ListView::Tree))
            .separator()
            .group_label("Group By")
            .item(group_item("None", ChangeGroup::None))
            .item(group_item(
                "Tracked & Untracked",
                ChangeGroup::TrackedUntracked,
            ))
            .item(group_item("Staged & Unstaged", ChangeGroup::StagedUnstaged))
            .on_close(|st: &mut State| st.editor.changes.menu = None)
            .build(&s.editor.kit, layer),
    )
}

/// Chevron + label + count; the whole area toggles the group.
fn group_toggle(key: &'static str, label: &str, count: usize, collapsed: bool, ink: &Ink) -> El {
    el().key(format!("group-toggle:{key}"))
        .grow(1.0)
        .min_w(0.0)
        .h(24.0)
        .px(4.0)
        .gap(4.0)
        .items_center()
        .radius(6.0)
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
/// accessible name keeps the full `orig → path`. `indent` is the `Tree`
/// view's extra left padding (0 in `List`).
fn entry_row(
    s: &State,
    ink: &Ink,
    k: &Kit,
    key: &'static str,
    layer: &'static str,
    entry: &app_core::git_view::StatusEntry,
    indent: f32,
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
        .pl(2.0 + indent)
        .gap(2.0)
        .items_center()
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .h(22.0)
                .px(4.0)
                .gap(4.0)
                .items_center()
                .radius(4.0)
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
                        .truncate_middle()
                        .help_when_truncated(name.clone()),
                )
                .child(
                    el().px(4.0)
                        .h(16.0)
                        .items_center()
                        .justify_center()
                        .radius(4.0)
                        .border(1.0, ink.border)
                        .child(theme::txt(status_label(entry.status), T11M_LH, ink.muted)),
                )
                .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                    activate_entry(st, layer, &sel_path, tx);
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

    #[test]
    fn large_group_truncates_at_cap_and_keeps_selection() {
        let (mut s, _home, _guard) = editor_scene();
        let total = MAX_GROUP_RENDER + 50;
        let mut d = changes();
        d.unstaged = (0..total)
            .map(|i| entry(&format!("src/file-{i:03}.rs"), None, "modified"))
            .collect();
        s.editor.changes.data = Some(d);
        // Pin the canonical flat layout: this test is about the cap.
        s.editor.list_view = ListView::List;
        s.editor.changes.group = ChangeGroup::None;
        // Select an entry past the MAX_GROUP_RENDER threshold.
        s.editor.changes.selected = Some(format!("src/file-{:03}.rs", total - 1));
        s.editor.changes.layer = Some("unstaged".into());

        let mut rt = Runtime::new(s, changes_only, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let ls = labels(&rt);
        // First entry is mounted.
        assert!(ls.iter().any(|l| l == "src/file-000.rs"));
        // Entry 199 is mounted.
        assert!(ls.iter().any(|l| l == "src/file-199.rs"));
        // Entry 205 (not selected) is truncated.
        assert!(!ls.iter().any(|l| l == "src/file-205.rs"));
        // Selected entry (249) is mounted despite being past the cap.
        assert!(
            ls.iter()
                .any(|l| l == &format!("src/file-{:03}.rs", total - 1))
        );
        // Truncation note is present.
        let note = "+50 more changed files (use filter to narrow)".to_string();
        let note_el = rt
            .elements()
            .into_iter()
            .find(|e| e.text.as_deref() == Some(&note));
        assert!(note_el.is_some(), "truncation note mounted: {ls:?}");
    }

    // -- List/Tree view + Group By -------------------------------------------

    #[test]
    fn list_groups_none_keeps_canonical_order_and_layers() {
        let d = changes();
        let groups = list_groups(&d, ChangeGroup::None);
        let keys: Vec<_> = groups.iter().map(|g| g.key).collect();
        assert_eq!(keys, ["conflicts", "staged", "unstaged", "untracked"]);
        assert_eq!(groups[1].label, "Staged");
        // Canonical rows keep their own group key and diff layer.
        assert_eq!(groups[1].entries[0].1, "staged");
        assert_eq!(groups[1].entries[0].2, "staged");
        assert_eq!(groups[3].entries[0].2, "untracked");
    }

    #[test]
    fn list_groups_tracked_untracked_keeps_row_layers() {
        let mut d = changes();
        d.conflicts = vec![entry("c.rs", None, "unmerged")];
        let groups = list_groups(&d, ChangeGroup::TrackedUntracked);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "tracked");
        assert_eq!(groups[0].label, "Tracked");
        let members: Vec<(&str, &str, &str)> = groups[0]
            .entries
            .iter()
            .map(|(e, k, l)| (e.path.as_str(), *k, *l))
            .collect();
        assert_eq!(
            members,
            [
                ("src/a.rs", "staged", "staged"),
                ("src/b.rs", "unstaged", "unstaged"),
                ("c.rs", "conflicts", "conflict"),
            ]
        );
        assert_eq!(groups[1].key, "untracked");
        assert_eq!(groups[1].entries.len(), 1);
    }

    #[test]
    fn list_groups_staged_unstaged_repeats_a_path_in_both() {
        let mut d = changes();
        d.staged.push(entry("dup.rs", None, "modified"));
        d.unstaged.push(entry("dup.rs", None, "modified"));
        let groups = list_groups(&d, ChangeGroup::StagedUnstaged);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "staged");
        assert_eq!(groups[1].key, "unstaged");
        // `dup.rs` renders under both headers, each row keeping its layer.
        assert!(
            groups[0]
                .entries
                .iter()
                .any(|(e, _, l)| e.path == "dup.rs" && *l == "staged")
        );
        assert!(
            groups[1]
                .entries
                .iter()
                .any(|(e, _, l)| e.path == "dup.rs" && *l == "unstaged")
        );
        // Untracked sits under Unstaged, still tagged for its own actions.
        assert!(
            groups[1]
                .entries
                .iter()
                .any(|(e, k, l)| e.path == "new.txt" && *k == "untracked" && *l == "untracked")
        );
    }

    #[test]
    fn list_groups_skips_empty_groups_when_grouping() {
        let mut d = changes();
        d.untracked.clear();
        let groups = list_groups(&d, ChangeGroup::TrackedUntracked);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "tracked");
        // `None` keeps the four canonical slots even when a slot is empty.
        assert_eq!(list_groups(&d, ChangeGroup::None).len(), 4);
    }

    #[test]
    fn tree_rows_nest_directories_before_files() {
        let files = vec![
            entry("src/a.rs", None, "modified"),
            entry("src/sub/b.rs", None, "modified"),
            entry("readme.md", None, "modified"),
        ];
        let refs: Vec<(&StatusEntry, ())> = files.iter().map(|e| (e, ())).collect();
        let rows = tree_rows(&refs, |_| false);
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match r {
                TreeRow::Dir { path, .. } => format!("d:{path}"),
                TreeRow::File { entry, .. } => format!("f:{}", entry.path),
            })
            .collect();
        // Each level emits its subdirs (natural order), then its files.
        assert_eq!(
            kinds,
            [
                "d:src",
                "d:src/sub",
                "f:src/sub/b.rs",
                "f:src/a.rs",
                "f:readme.md"
            ]
        );
        let depths: Vec<usize> = rows
            .iter()
            .map(|r| match r {
                TreeRow::Dir { depth, .. } | TreeRow::File { depth, .. } => *depth,
            })
            .collect();
        assert_eq!(depths, [0, 1, 2, 1, 0]);
    }

    #[test]
    fn tree_rows_skip_a_collapsed_directory() {
        let files = vec![
            entry("src/a.rs", None, "modified"),
            entry("x.rs", None, "modified"),
        ];
        let refs: Vec<(&StatusEntry, ())> = files.iter().map(|e| (e, ())).collect();
        let rows = tree_rows(&refs, |p| p == "src");
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], TreeRow::Dir { .. }));
        assert!(matches!(rows[1], TreeRow::File { .. }));
    }

    #[test]
    fn changes_state_defaults_and_reset_keeps_layout() {
        let mut c = crate::editor::state::ChangesState::empty();
        assert_eq!(c.group, ChangeGroup::StagedUnstaged);
        assert!(c.menu.is_none());

        c.group = ChangeGroup::StagedUnstaged;
        c.selected = Some("x".into());
        c.collapsed.insert("dir:src".into());
        c.reset();
        assert_eq!(c.group, ChangeGroup::StagedUnstaged);
        assert!(c.selected.is_none());
        assert!(c.collapsed.is_empty());
        // The List/Tree choice is shared (`EditorState::list_view`), so a
        // checkout switch cannot reset it here.
        assert_eq!(ListView::default(), ListView::Tree);
    }

    /// Right-click on the changed-file list opens its View/Group-By menu
    /// with the active choices checked — the menu is list-level, so a press
    /// over a file row still names the list.
    #[test]
    fn secondary_press_opens_the_list_menu() {
        let (s, _home, _guard) = editor_scene();
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let row = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "button" && e.label.as_deref() == Some("src/staged-00.rs"))
            .expect("staged row mounted");
        let (x, y) = (
            row.rect[0] + row.rect[2] / 2.0,
            row.rect[1] + row.rect[3] / 2.0,
        );
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);

        let m = rt
            .state
            .editor
            .changes
            .menu
            .as_ref()
            .expect("secondary press opens the list menu");
        assert_eq!(*m, ListMenu::Point { x, y });

        rt.frame(1.0 / 60.0);
        let item = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "menuitemcheckbox" && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("menu item {label:?} mounted"))
        };
        for l in [
            "List",
            "Tree",
            "None",
            "Tracked & Untracked",
            "Staged & Unstaged",
        ] {
            item(&rt, l);
        }
        assert!(item(&rt, "Tree").item_selected, "Tree is the active view");
        assert!(
            item(&rt, "Staged & Unstaged").item_selected,
            "Staged & Unstaged is the active grouping"
        );
        assert!(!item(&rt, "List").item_selected);
    }

    /// Picking a menu item writes the choice, closes the menu, and the new
    /// layout renders — dir rows for `Tree`, composite headers for a group.
    #[test]
    fn menu_items_switch_view_and_group() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.changes.menu = Some(ListMenu::Point { x: 100.0, y: 100.0 });
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let pick = |rt: &mut Runtime<State>, label: &str| {
            let it = rt
                .elements()
                .into_iter()
                .find(|e| !e.exiting && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("menu item {label:?} mounted"));
            click(
                rt,
                it.rect[0] + it.rect[2] / 2.0,
                it.rect[1] + it.rect[3] / 2.0,
            );
        };
        pick(&mut rt, "Tree");
        assert_eq!(rt.state.editor.list_view, ListView::Tree);
        assert!(rt.state.editor.changes.menu.is_none(), "picking closes");

        rt.frame(1.0 / 60.0);
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.label.as_deref() == Some("src directory")),
            "src dir row mounts in Tree view"
        );
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.label.as_deref() == Some("misc directory")),
            "misc dir row mounts in Tree view"
        );

        rt.state.editor.changes.menu = Some(ListMenu::Point { x: 100.0, y: 100.0 });
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        pick(&mut rt, "Staged & Unstaged");
        assert_eq!(rt.state.editor.changes.group, ChangeGroup::StagedUnstaged);

        rt.frame(1.0 / 60.0);
        // Assert on the group-toggle keys — the "Untracked" text would also
        // match the untracked rows' status badges.
        let keys: Vec<String> = rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.key)
            .collect();
        assert!(keys.iter().any(|k| k == "group-toggle:staged"));
        assert!(keys.iter().any(|k| k == "group-toggle:unstaged"));
        assert!(
            !keys
                .iter()
                .any(|k| k == "group-toggle:untracked" || k == "group-toggle:conflicts"),
            "composite grouping renders only Staged and Unstaged headers"
        );
    }

    /// In `Tree` view a directory row toggles its subtree in
    /// `changes.collapsed` under the `dir:` key, leaving the group collapse
    /// keys untouched.
    #[test]
    fn tree_view_dir_rows_expand_and_collapse() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.list_view = ListView::Tree;
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let ls = labels(&rt);
        assert!(ls.iter().any(|l| l == "src/staged-00.rs"));

        let dir = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("src directory"))
            .expect("src dir row mounted");
        click(&mut rt, dir.rect[0] + 40.0, dir.rect[1] + dir.rect[3] / 2.0);

        assert!(
            rt.state.editor.changes.collapsed.contains(&dir_key("src")),
            "the dir row records its collapse"
        );
        rt.frame(1.0 / 60.0);
        // Collapsed-away rows linger a few frames as exiting elements.
        let ls: Vec<String> = rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.label)
            .collect();
        assert!(
            !ls.iter().any(|l| l == "src/staged-00.rs"),
            "collapsed dir hides its files"
        );
    }

    /// An empty list still opens the menu — the press lands on the scroll
    /// area itself.
    #[test]
    fn the_list_menu_opens_on_an_empty_list() {
        let (mut s, _home, _guard) = editor_scene();
        s.editor.changes.data = Some(app_core::git_view::GitViewChanges {
            branch: Some("main".into()),
            head: "abc".into(),
            conflicts: vec![],
            staged: vec![],
            unstaged: vec![],
            untracked: vec![],
            ahead: None,
        });
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // An all-empty worktree shows the clean pane in place of the list;
        // the menu still opens on it.
        let clean = rt
            .elements()
            .into_iter()
            .find(|e| e.text.as_deref() == Some("Working tree clean"))
            .expect("clean pane mounted");
        let (x, y) = (
            clean.rect[0] + clean.rect[2] / 2.0,
            clean.rect[1] + clean.rect[3] / 2.0,
        );
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.editor.changes.menu.is_some(),
            "menu opens with no rows"
        );
    }

    /// A path staged AND unstaged appears in both composite groups; picking
    /// each row selects its own layer.
    #[test]
    fn grouped_view_keeps_each_rows_layer() {
        let (mut s, _home, _guard) = editor_scene();
        let mut d = changes();
        d.staged = vec![entry("dup.rs", None, "modified")];
        d.unstaged = vec![entry("dup.rs", None, "modified")];
        d.untracked = vec![];
        s.editor.changes.data = Some(d);
        s.editor.changes.group = ChangeGroup::StagedUnstaged;
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let rows: Vec<rune::runtime::ElementInfo> = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "button" && e.label.as_deref() == Some("dup.rs"))
            .collect();
        assert_eq!(rows.len(), 2, "dup.rs renders in both groups");

        for r in rows {
            click(
                &mut rt,
                r.rect[0] + r.rect[2] / 2.0,
                r.rect[1] + r.rect[3] / 2.0,
            );
        }
        // The second click lands on the Unstaged copy (the Staged header
        // renders first), so the layer identifies which row won last.
        assert_eq!(
            rt.state.editor.changes.layer.as_deref(),
            Some("unstaged"),
            "each row selects its own layer"
        );
        assert_eq!(rt.state.editor.changes.selected.as_deref(), Some("dup.rs"));
    }

    /// The header's view button opens the same View/Group-By menu as the
    /// secondary press, anchored under the button instead of at a point.
    #[test]
    fn the_view_button_opens_the_list_menu() {
        let (s, _home, _guard) = editor_scene();
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let button = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("View and group changed files"))
            .expect("view button mounted");
        click(
            &mut rt,
            button.rect[0] + button.rect[2] / 2.0,
            button.rect[1] + button.rect[3] / 2.0,
        );
        assert_eq!(
            rt.state.editor.changes.menu,
            Some(ListMenu::Anchor),
            "the button opens the anchored menu"
        );

        rt.frame(1.0 / 60.0);
        let item = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "menuitemcheckbox" && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("menu item {label:?} mounted"))
        };
        assert!(
            item(&rt, "Tree").item_selected,
            "Tree is checked by default"
        );
        assert!(
            item(&rt, "Staged & Unstaged").item_selected,
            "Staged & Unstaged is checked by default"
        );

        let list = item(&rt, "List");
        click(
            &mut rt,
            list.rect[0] + list.rect[2] / 2.0,
            list.rect[1] + list.rect[3] / 2.0,
        );
        assert_eq!(rt.state.editor.list_view, ListView::List);
        assert_eq!(
            rt.state.editor.changes.menu, None,
            "a choice closes the menu"
        );
    }

    /// Reopening from the button after a choice still names the active
    /// options, and the group choice applies the same way.
    #[test]
    fn the_view_button_menu_applies_group_choices() {
        let (s, _home, _guard) = editor_scene();
        let mut rt = Runtime::new(s, changes_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let button = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("View and group changed files"))
            .expect("view button mounted");
        let (bx, by) = (
            button.rect[0] + button.rect[2] / 2.0,
            button.rect[1] + button.rect[3] / 2.0,
        );
        click(&mut rt, bx, by);
        rt.frame(1.0 / 60.0);
        let grouped = rt
            .elements()
            .into_iter()
            .find(|e| {
                e.role == "menuitemcheckbox" && e.label.as_deref() == Some("Staged & Unstaged")
            })
            .expect("group item mounted");
        click(
            &mut rt,
            grouped.rect[0] + grouped.rect[2] / 2.0,
            grouped.rect[1] + grouped.rect[3] / 2.0,
        );
        assert_eq!(
            rt.state.editor.changes.group,
            ChangeGroup::StagedUnstaged,
            "the button's menu applies the group choice"
        );
        assert_eq!(rt.state.editor.changes.menu, None);

        // Reopening marks the current group.
        click(&mut rt, bx, by);
        rt.frame(1.0 / 60.0);
        let again = rt
            .elements()
            .into_iter()
            .find(|e| {
                e.role == "menuitemcheckbox" && e.label.as_deref() == Some("Staged & Unstaged")
            })
            .expect("group item mounted again");
        assert!(again.item_selected, "the active group is checked");
    }
}

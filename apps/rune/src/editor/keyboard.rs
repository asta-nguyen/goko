//! Global editor keyboard handling, ported from the desktop workspace's
//! window listener: command chords (⌘S save, contextual ⌘F / ⇧⌘F, ⌘P
//! quick-open), `1`–`4` section switching and `j`/`k` list navigation with
//! Enter activation.
//!
//! Command chords fire from anywhere in the editor view — the desktop saves
//! and finds even while a field is focused. Plain-key shortcuts (digits,
//! j/k, Enter) are gated on the same guards the desktop applies: a checkout
//! must be selected, no editable field may hold focus or an IME composition,
//! and no modal surface (draft guard, pickers, context menu, quick-open)
//! may own input. `j`/`k` only move a highlight cursor — they never open
//! files, select commits or mutate Git state; Enter activates through each
//! row's existing handler.

use rune::edit::TextEdit;
use rune::{Key, KeyEvent, Tx};

use crate::editor::changes::{self, GROUPS};
use crate::editor::files;
use crate::editor::history::{self, UNCOMMITTED_ID};
use crate::editor::navigation::{self, PendingNav};
use crate::editor::quick_open;
use crate::editor::search;
use crate::editor::state::Section;
use crate::{State, View};

/// Keyboard-owned state on `EditorState`: the highlight-only list cursor.
#[derive(Clone, Debug, Default)]
pub struct KeyboardState {
    /// Current j/k position; `None` until the first move/activation.
    pub nav: Option<NavPos>,
}

/// One position in the current section's [`nav_items`] order.
#[derive(Clone, Debug)]
pub struct NavPos {
    pub section: Section,
    pub index: usize,
    /// Canonical row key ([`NavItem::key`]) so `nav_active` styling survives
    /// list refreshes that shift indexes.
    pub key: String,
}

/// App-level entry from `AppSpec::on_key`, chained before the LSP F12/Esc
/// logic in `main.rs`. Returns `true` when the key was consumed.
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.view != View::Editor || modal(s) {
        return false;
    }
    if quick_open::chord(s, ev) {
        return true;
    }
    if ev.is_command('s') {
        files::save_active_doc(s);
        return true;
    }
    if ev.is_command('f') {
        if ev.mods.shift {
            open_content_search(s, tx);
            return true;
        }
        return find(s, tx);
    }
    if !plain_key(s, ev) {
        return false;
    }
    match &ev.key {
        Key::Text(t) if t == "j" => nav_move(s, 1, tx),
        Key::Text(t) if t == "k" => nav_move(s, -1, tx),
        Key::Enter => nav_activate(s, tx),
        Key::Text(t) if t.len() == 1 => match t.as_bytes()[0] {
            b'1'..=b'4' => section_digit(s, t.as_bytes()[0] - b'0', tx),
            _ => false,
        },
        _ => false,
    }
}

/// Modal surfaces that own every key: draft guard, project/checkout pickers,
/// history branch picker, agent picker, row context menu, quick-open.
fn modal(s: &State) -> bool {
    let e = &s.editor;
    e.guard.is_some()
        || e.project_picker_open
        || e.checkout_picker_open
        || e.history.branch_picker.open
        || e.commit.agent_picker_open
        || e.history.row_menu.is_some()
        || e.quick_open.open
}

/// A checkout is selected — the same `ready` gate the desktop applies
/// before section/list shortcuts mean anything.
fn ready(s: &State) -> bool {
    s.editor.checkout_key.is_some()
}

/// The editable text fields the CURRENT section has mounted. Unmounted
/// sections keep stale `focused`/`preedit` flags (nothing syncs them while
/// hidden), so typing is judged per-section, never globally. Servers' own
/// command-override field is private to `editor::lsp`; that section has no
/// nav items, so the guard needs no entry for it.
fn mounted_editables(s: &State) -> Vec<&TextEdit> {
    let e = &s.editor;
    let mut v: Vec<&TextEdit> = Vec::new();
    match e.section {
        Section::Files => {
            v.push(&e.files.filter);
            if e.files.search.open {
                v.push(&e.files.search_edit);
            }
            if let Some(d) = e.doc.as_ref().filter(|d| d.editable()) {
                v.push(&d.code.edit);
                if d.code.find.open {
                    v.push(&d.code.find.query);
                }
            }
        }
        Section::Changes => {
            v.push(&e.changes.filter);
            v.push(&e.commit.message);
        }
        Section::History => v.push(&e.history.filter),
        Section::Stashes => v.push(&e.stashes.filter),
        Section::Servers => {}
    }
    if e.quick_open.open {
        v.push(&e.quick_open.query);
    }
    v
}

fn mounted_editables_mut(s: &mut State) -> Vec<&mut TextEdit> {
    let e = &mut s.editor;
    let mut v: Vec<&mut TextEdit> = Vec::new();
    match e.section {
        Section::Files => {
            v.push(&mut e.files.filter);
            if e.files.search.open {
                v.push(&mut e.files.search_edit);
            }
            if let Some(d) = e.doc.as_mut().filter(|d| d.editable()) {
                v.push(&mut d.code.edit);
                if d.code.find.open {
                    v.push(&mut d.code.find.query);
                }
            }
        }
        Section::Changes => {
            v.push(&mut e.changes.filter);
            v.push(&mut e.commit.message);
        }
        Section::History => v.push(&mut e.history.filter),
        Section::Stashes => v.push(&mut e.stashes.filter),
        Section::Servers => {}
    }
    if e.quick_open.open {
        v.push(&mut e.quick_open.query);
    }
    v
}

/// True while a mounted editable holds keyboard focus or an IME composition
/// — plain-key shortcuts must not steal text or preedit.
fn typing(s: &State) -> bool {
    mounted_editables(s)
        .iter()
        .any(|e| e.focused || e.preedit.is_some())
}

/// Clear `focused` on every mounted editable so a model-driven focus request
/// (the only focus API Rune exposes) lands deterministically.
pub fn clear_edit_focuses(s: &mut State) {
    for e in mounted_editables_mut(s) {
        e.focused = false;
    }
}

fn focus_text(e: &mut TextEdit) {
    e.focused = true;
    let n = e.text.len();
    e.set_selection(0, n);
}

/// Plain-key gate shared by digits/j/k/Enter: checkout selected, nothing
/// typing or composing, no command/option modifiers.
fn plain_key(s: &State, ev: &KeyEvent) -> bool {
    ready(s) && !typing(s) && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.logo
}

/// Keys the Files tree yields to global nav before its own typeahead:
/// `j`/`k` and the section digits `1`–`4`. Everything else — letters,
/// arrows, Enter — keeps the tree's native dispatch. The tree's composite
/// handler calls this, then `on_key`; a `false` from `on_key` falls back to
/// native typeahead so an empty nav list can't swallow the key.
pub(crate) fn preempt_tree_key(s: &State, ev: &KeyEvent) -> bool {
    plain_key(s, ev)
        && matches!(&ev.key, Key::Text(t) if t == "j" || t == "k" || (t.len() == 1 && matches!(t.as_bytes()[0], b'1'..=b'4')))
}

/// ⌘F: the code surface's own find handler consumes the chord while its
/// field is focused (element handlers run before `on_key`); anywhere else
/// focus the current section's filter. In-file find open with its bar
/// focused keeps the user in the code context.
fn find(s: &mut State, _tx: &mut Tx) -> bool {
    if !ready(s) {
        return false;
    }
    if s.editor.section == Section::Files
        && let Some(d) = s.editor.doc.as_ref().filter(|d| d.editable())
    {
        if d.code.find.open && d.code.find.query.focused {
            if let Some(d) = s.editor.doc.as_mut() {
                d.code.find.query.focused = false;
                focus_text(&mut d.code.edit);
            }
            return true;
        }
        if d.code.edit.focused {
            // The field's own handler owns ⌘F; if it declined, do nothing.
            return false;
        }
    }
    focus_section_filter(s);
    true
}

/// ⇧⌘F: open (or refocus) the Files content search — the existing
/// `search_edit`/`Req::Search` path, no new backend surface. The section
/// switch goes through the same guarded navigation the tabs use.
fn open_content_search(s: &mut State, tx: &mut Tx) {
    if !ready(s) {
        return;
    }
    s.editor.files.search.open = true;
    clear_edit_focuses(s);
    focus_text(&mut s.editor.files.search_edit);
    if s.editor.section != Section::Files {
        navigation::navigate(s, PendingNav::SetSection(Section::Files), tx);
    }
    search::on_query_change(s, tx);
}

/// Focus the current section's filter input with its text selected.
fn focus_section_filter(s: &mut State) -> bool {
    clear_edit_focuses(s);
    match s.editor.section {
        Section::Files => focus_text(&mut s.editor.files.filter),
        Section::Changes => focus_text(&mut s.editor.changes.filter),
        Section::History => focus_text(&mut s.editor.history.filter),
        Section::Stashes => focus_text(&mut s.editor.stashes.filter),
        Section::Servers => return false,
    }
    true
}

/// `1`–`4` → Files/Changes/History/Stashes, routed through the same guarded
/// `PendingNav::SetSection` the section tabs use.
fn section_digit(s: &mut State, d: u8, tx: &mut Tx) -> bool {
    if !ready(s) || !(1..=4).contains(&d) {
        return false;
    }
    let target = Section::ALL[(d - 1) as usize];
    if s.editor.section == target {
        return true;
    }
    navigation::navigate(s, PendingNav::SetSection(target), tx);
    true
}

// ---- j/k list navigation -------------------------------------------------

/// Activatable row identity, in display order for the current section.
#[derive(Clone, Debug)]
pub(crate) enum NavItem {
    /// Files tree node id — also its checkout-relative path.
    FileTree(String),
    /// Files content-search result row.
    SearchMatch { path: String, line: u64 },
    /// Changes group entry (layer key + entry path).
    Change { layer: &'static str, path: String },
    /// History row id: commit oid or the uncommitted sentinel.
    History(String),
    /// Stashes row id: stash oid.
    Stash(String),
}

/// Canonical element key for a Changes entry row — row builders set this as
/// `.key()` so `nav_active`/`Tx::focused` can find it (see the seams note).
pub(crate) fn change_row_key(layer: &str, path: &str) -> String {
    format!("goko-nav-change:{layer}:{path}")
}

/// Canonical element key for a Stashes row.
pub(crate) fn stash_row_key(id: &str) -> String {
    format!("goko-nav-stash:{id}")
}

/// Canonical element key for a Files content-search result row.
pub(crate) fn search_row_key(path: &str, line: u64) -> String {
    format!("goko-nav-search:{path}:{line}")
}

/// Scroll scope each section's list should mount under this key so
/// `nav_reveal` can bring the cursor row into view.
pub(crate) fn nav_scroll_scope(section: Section) -> &'static str {
    match section {
        Section::Files => "goko-nav-scroll-files",
        Section::Changes => "goko-nav-scroll-changes",
        Section::History => "goko-nav-scroll-history",
        Section::Stashes => "goko-nav-scroll-stashes",
        Section::Servers => "goko-nav-scroll-servers",
    }
}

impl NavItem {
    /// The key `nav_active` compares and `Tx::focused` reports once rows
    /// carry these keys. FileTree/History reuse ids the widgets already key.
    fn key(&self) -> String {
        match self {
            Self::FileTree(id) | Self::History(id) => id.clone(),
            Self::SearchMatch { path, line } => search_row_key(path, *line),
            Self::Change { layer, path } => change_row_key(layer, path),
            Self::Stash(id) => stash_row_key(id),
        }
    }
}

/// Row builders call this to paint the j/k highlight — distinct from each
/// section's own selected-row styling, which stays untouched.
pub(crate) fn nav_active(s: &State, section: Section, row_key: &str) -> bool {
    s.editor
        .keyboard
        .nav
        .as_ref()
        .is_some_and(|p| p.section == section && p.key == row_key)
}

/// The current section's activatable rows, in display order. Mirrors each
/// view's own filtering/collapse rules so j/k lands exactly where the eye
/// sees a row.
fn nav_items(s: &State) -> Vec<NavItem> {
    let e = &s.editor;
    match e.section {
        Section::Files => {
            if e.files.search.open {
                return e
                    .files
                    .search
                    .results
                    .as_ref()
                    .map(|r| {
                        r.matches
                            .iter()
                            .take(500)
                            .map(|m| NavItem::SearchMatch {
                                path: m.path.clone(),
                                line: m.line,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            let m = &e.files.model;
            (0..m.visible_len())
                .filter_map(|i| m.visible_id(i).map(str::to_string))
                .map(NavItem::FileTree)
                .collect()
        }
        Section::Changes => {
            if e.changes.checkout != e.checkout_key {
                return Vec::new();
            }
            let Some(data) = e.changes.data.as_ref() else {
                return Vec::new();
            };
            let filter = e.changes.filter.text.trim();
            let mut out = Vec::new();
            for (key, layer) in GROUPS {
                if e.changes.collapsed.contains(key) {
                    continue;
                }
                for entry in changes::group_entries(data, key) {
                    if !filter.is_empty()
                        && !changes::matches_filter(&changes::entry_name(entry), filter)
                    {
                        continue;
                    }
                    out.push(NavItem::Change {
                        layer,
                        path: entry.path.clone(),
                    });
                }
            }
            out
        }
        Section::History => {
            let mut out = Vec::new();
            if uncommitted_row_visible(s) {
                out.push(NavItem::History(UNCOMMITTED_ID.to_string()));
            }
            out.extend(
                e.history
                    .commits
                    .iter()
                    .map(|c| NavItem::History(c.id.clone())),
            );
            out
        }
        Section::Stashes => {
            let h = &e.stashes;
            let q = h.filter.text.trim();
            h.stashes
                .iter()
                .filter(|it| {
                    q.is_empty()
                        || history::matches_query(
                            &format!("{} {}", it.message, it.branch.as_deref().unwrap_or("")),
                            q,
                        )
                })
                .map(|it| NavItem::Stash(it.id.clone()))
                .collect()
        }
        Section::Servers => Vec::new(),
    }
}

/// Desktop rule, mirrored from `history::uncommitted_visible`: the row
/// shows when entries exist AND a HEAD commit is loaded.
fn uncommitted_row_visible(s: &State) -> bool {
    if !history::changes_fresh(s) {
        return false;
    }
    let Some(data) = s.editor.changes.data.as_ref() else {
        return false;
    };
    let n = data.conflicts.len() + data.staged.len() + data.unstaged.len() + data.untracked.len();
    n > 0 && s.editor.history.commits.iter().any(|c| c.head)
}

/// j/k: move the cursor one row (clamped), reseeding from real focus when a
/// nav-keyed row holds it, then reveal. Never selects or opens anything.
fn nav_move(s: &mut State, delta: isize, tx: &mut Tx) -> bool {
    if !ready(s) {
        return false;
    }
    let items = nav_items(s);
    if items.is_empty() {
        s.editor.keyboard.nav = None;
        return false;
    }
    let section = s.editor.section;
    let cur = tx
        .focused()
        .and_then(|f| items.iter().position(|i| i.key() == f))
        .or_else(|| {
            s.editor
                .keyboard
                .nav
                .as_ref()
                .filter(|p| p.section == section)
                .map(|p| p.index)
        });
    let len = items.len() as isize;
    let next = match cur {
        Some(i) => (i as isize + delta).clamp(0, len - 1) as usize,
        None => {
            if delta > 0 {
                0
            } else {
                (len - 1) as usize
            }
        }
    };
    let item = items[next].clone();
    s.editor.keyboard.nav = Some(NavPos {
        section,
        index: next,
        key: item.key(),
    });
    nav_reveal(s, &item, tx);
    true
}

/// Scroll/expand the cursor row into view using each widget's own reveal —
/// no selection callbacks fire (model-level calls, not input events).
fn nav_reveal(s: &mut State, item: &NavItem, tx: &mut Tx) {
    match item {
        NavItem::FileTree(id) => {
            s.editor.files.model.reveal(id, tx);
        }
        NavItem::SearchMatch { path, line } => {
            tx.scroll_into_view_in(
                nav_scroll_scope(Section::Files),
                search_row_key(path, *line),
            );
        }
        NavItem::Change { layer, path } => {
            tx.scroll_into_view_in(
                nav_scroll_scope(Section::Changes),
                change_row_key(layer, path),
            );
        }
        NavItem::History(id) => {
            s.editor.history.table.reveal(id.clone(), tx);
        }
        NavItem::Stash(id) => {
            tx.scroll_into_view_in(nav_scroll_scope(Section::Stashes), stash_row_key(id));
        }
    }
}

/// Enter: activate the cursor row through the same path its own press uses.
fn nav_activate(s: &mut State, tx: &mut Tx) -> bool {
    if !ready(s) {
        return false;
    }
    let section = s.editor.section;
    let Some(pos) = s.editor.keyboard.nav.clone() else {
        return false;
    };
    if pos.section != section {
        return false;
    }
    let items = nav_items(s);
    // Lists refresh under the cursor: prefer the stored key, fall back to
    // the stored index for keyless drift.
    let item = items
        .iter()
        .find(|i| i.key() == pos.key)
        .or_else(|| items.get(pos.index))
        .cloned();
    let Some(item) = item else {
        return false;
    };
    match item {
        NavItem::FileTree(id) => {
            let is_dir = s.editor.files.model.get(&id).is_some_and(|n| n.dir);
            if is_dir {
                s.editor.files.model.toggle(&id);
            } else {
                files::open_doc(s, &id, None, tx);
            }
        }
        NavItem::SearchMatch { path, line } => {
            files::open_doc(s, &path, Some(line), tx);
        }
        NavItem::Change { layer, path } => {
            changes::select_entry(&mut s.editor, layer, &path);
        }
        NavItem::History(id) => {
            if id == UNCOMMITTED_ID {
                // `select_uncommitted` is private to history.rs; same four
                // assignments its body makes.
                s.editor.history.selected = Some(UNCOMMITTED_ID.to_string());
                s.editor.history.detail = None;
                s.editor.history.detail_loading = false;
                s.editor.history.table.select(UNCOMMITTED_ID.to_string());
            } else {
                history::select_commit_pub(s, &id, tx);
            }
        }
        NavItem::Stash(id) => {
            if s.editor.stashes.selected.as_deref() != Some(id.as_str()) {
                s.editor.stashes.selected = Some(id.clone());
                s.editor.stashes.detail = None;
                // Same enqueue `stashes::enqueue_stash_diff` performs.
                if let (Some(checkout), Some(key)) =
                    (s.editor.checkout.clone(), s.editor.checkout_key.clone())
                {
                    s.editor.stashes.detail_loading = true;
                    if !s.editor.enqueue_read(
                        checkout,
                        key,
                        crate::editor::backend::Req::StashDiff { stash: id.clone() },
                    ) {
                        s.editor.stashes.detail_loading = false;
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, El, Env, Runtime, col};

    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = std::path::PathBuf::from("/tmp/goko-kb-qa");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.section = Section::Files;
        s
    }

    fn runtime() -> Runtime<State> {
        let mut rt = Runtime::new(scene(), view, (900.0, 700.0), Env::new(true, 2.0));
        rt.on_key = Some(on_key);
        rt
    }

    fn view(s: &State, _cx: &mut Cx<State>) -> El {
        let _ = s;
        col().grow(1.0).h_full()
    }

    fn cmd(c: char) -> KeyEvent {
        KeyEvent {
            key: Key::Command(c),
            mods: rune::Mods {
                cmd: true,
                ..Default::default()
            },
        }
    }

    fn text(t: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Text(t.to_string()),
            mods: Default::default(),
        }
    }

    #[test]
    fn cmd_s_consumed_without_doc() {
        let mut rt = runtime();
        rt.frame(0.0);
        rt.key(cmd('s'));
        rt.frame(0.0);
        // Consumed (no crash, no doc to save).
    }

    #[test]
    fn cmd_f_focuses_section_filter() {
        let mut rt = runtime();
        rt.frame(0.0);
        rt.key(cmd('f'));
        rt.frame(0.0);
        rt.frame(0.0);
        assert!(rt.state.editor.files.filter.focused, "files filter focused");
    }

    #[test]
    fn digits_switch_section() {
        let mut rt = runtime();
        rt.frame(0.0);
        rt.key(text("2"));
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!(rt.state.editor.section, Section::Changes);
        rt.key(text("1"));
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!(rt.state.editor.section, Section::Files);
    }

    #[test]
    fn typing_guard_blocks_plain_keys() {
        let mut rt = runtime();
        rt.state.editor.files.filter.focused = true;
        rt.key(text("2"));
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!(
            rt.state.editor.section,
            Section::Files,
            "digit must not switch sections while typing"
        );
        // Command chords still work while typing.
        rt.key(cmd('f'));
        rt.frame(0.0);
        rt.frame(0.0);
        assert!(rt.state.editor.files.filter.focused);
    }

    #[test]
    fn ime_preedit_blocks_plain_keys() {
        let mut rt = runtime();
        rt.state.editor.files.filter.preedit = Some(("か".into(), None));
        rt.key(text("2"));
        rt.key(text("j"));
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!(rt.state.editor.section, Section::Files);
        assert!(rt.state.editor.keyboard.nav.is_none());
    }

    #[test]
    fn j_k_move_cursor_without_opening() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec!["a.rs".into(), "b.rs".into(), "c.rs".into()];
        rt.state.editor.rebuild_file_model();
        rt.frame(0.0);
        rt.key(text("j"));
        rt.frame(0.0);
        let nav = rt.state.editor.keyboard.nav.clone().expect("nav engaged");
        assert_eq!(nav.index, 0);
        assert!(rt.state.editor.doc.is_none(), "j/k never opens a doc");
        rt.key(text("j"));
        rt.frame(0.0);
        assert_eq!(rt.state.editor.keyboard.nav.as_ref().unwrap().index, 1);
        rt.key(text("k"));
        rt.frame(0.0);
        assert_eq!(rt.state.editor.keyboard.nav.as_ref().unwrap().index, 0);
    }

    #[test]
    fn enter_on_tree_row_opens_doc() {
        let mut rt = runtime();
        rt.state.editor.files.paths = vec!["a.rs".into()];
        rt.state.editor.rebuild_file_model();
        rt.frame(0.0);
        rt.key(text("j"));
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Default::default(),
        });
        rt.frame(0.0);
        rt.frame(0.0);
        // Doc load is backend-async; the request must at least be pending
        // on the right path rather than dropped.
        assert!(
            rt.state.editor.doc_load.is_some() || rt.state.editor.doc.is_some(),
            "enter queued the open"
        );
    }

    // ---- section row seams -------------------------------------------------

    use crate::editor::{changes, stashes};
    use app_core::git_view::{
        GitViewChanges, GitViewSearch, GitViewSearchMatch, GitViewStash, StatusEntry,
    };

    /// Mounts the real section view for whatever `s.editor.section` is, so
    /// row `.key()`/nav-fill/scroll wiring is exercised end to end.
    fn section_view(s: &State, _cx: &mut Cx<State>) -> El {
        match s.editor.section {
            Section::Changes => changes::view(s),
            Section::Stashes => stashes::view(s),
            Section::Files => files::files_view(s, &s.ink, &s.editor.kit),
            _ => col().grow(1.0),
        }
    }

    fn section_runtime(s: State) -> Runtime<State> {
        let mut rt = Runtime::new(s, section_view, (900.0, 700.0), Env::new(true, 2.0));
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt
    }

    fn status_entry(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            status: "modified",
            added: None,
            removed: None,
        }
    }

    /// The mounted element carrying `key`, with its effective fill.
    fn keyed(rt: &Runtime<State>, key: &str) -> Option<rune::ElementInfo> {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
    }

    fn press_j(rt: &mut Runtime<State>, n: usize) {
        for _ in 0..n {
            rt.key(text("j"));
            rt.frame(0.0);
        }
        // Let the scroll glide settle before reading row visibility.
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
    }

    /// Changes: j/k moves the nav cursor onto real row keys, the row paints
    /// the nav fill, the list scrolls it into view, and Enter selects through
    /// the same `select_entry` the row press uses.
    #[test]
    fn changes_jk_highlight_scroll_enter() {
        let mut s = scene();
        s.editor.section = Section::Changes;
        let unstaged: Vec<StatusEntry> = (0..30)
            .map(|i| status_entry(&format!("src/f{i:02}.rs")))
            .collect();
        s.editor.changes.data = Some(GitViewChanges {
            branch: None,
            head: "h".into(),
            conflicts: vec![],
            staged: vec![],
            unstaged,
            untracked: vec![],
            ahead: None,
        });
        s.editor.changes.checkout = s.editor.checkout_key.clone();
        let mut rt = section_runtime(s);

        // Scope + first row keys are mounted.
        assert!(keyed(&rt, nav_scroll_scope(Section::Changes)).is_some());
        let first = change_row_key("unstaged", "src/f00.rs");
        assert!(keyed(&rt, &first).is_some(), "row key mounted");

        press_j(&mut rt, 1);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 0);
        let row = keyed(&rt, &first).expect("nav row mounted");
        assert_eq!(
            row.fill,
            rt.state.ink.accent.alpha(0.14),
            "nav cursor paints its own fill"
        );
        // Selected-row styling is a different state: nothing selected yet.
        assert!(rt.state.editor.changes.selected.is_none());

        // Deep enough to overflow the viewport, the cursor row scrolls in.
        press_j(&mut rt, 29);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 29);
        let last_key = change_row_key("unstaged", "src/f29.rs");
        let row = keyed(&rt, &last_key).expect("last row mounted");
        assert!(
            row.visible[3] > row.rect[3] * 0.5,
            "scrolled into view: visible={:?} rect={:?}",
            row.visible,
            row.rect
        );

        // Enter activates through select_entry — the row press's own path.
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Default::default(),
        });
        rt.frame(0.0);
        assert_eq!(
            rt.state.editor.changes.selected.as_deref(),
            Some("src/f29.rs")
        );
        assert_eq!(rt.state.editor.changes.layer.as_deref(), Some("unstaged"));
    }

    /// Stashes: same contract — keyed rows, nav fill, scroll, Enter selects
    /// the stash (the row press's own path).
    #[test]
    fn stashes_jk_highlight_scroll_enter() {
        let mut s = scene();
        s.editor.section = Section::Stashes;
        s.editor.stashes.stashes = (0..30)
            .map(|i| GitViewStash {
                index: i,
                id: format!("stash{i}"),
                message: format!("WIP on main: change {i}"),
                branch: Some("main".into()),
                time: 1_700_000_000_000 + i as u64,
            })
            .collect();
        let mut rt = section_runtime(s);

        assert!(keyed(&rt, nav_scroll_scope(Section::Stashes)).is_some());
        let first = stash_row_key("stash0");
        assert!(keyed(&rt, &first).is_some(), "row key mounted");

        press_j(&mut rt, 1);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 0);
        let row = keyed(&rt, &first).expect("nav row mounted");
        assert_eq!(row.fill, rt.state.ink.accent.alpha(0.14));

        press_j(&mut rt, 29);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 29);
        let last_key = stash_row_key("stash29");
        let row = keyed(&rt, &last_key).expect("last row mounted");
        assert!(
            row.visible[3] > row.rect[3] * 0.5,
            "scrolled into view: visible={:?} rect={:?}",
            row.visible,
            row.rect
        );

        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Default::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.editor.stashes.selected.as_deref(), Some("stash29"));
    }

    /// Content-search results: j/k highlights + scrolls match rows; Enter
    /// routes through `open_doc` like the row press.
    #[test]
    fn search_results_jk_highlight_scroll_enter() {
        let mut s = scene();
        s.editor.section = Section::Files;
        s.editor.files.search.open = true;
        s.editor.files.search.results = Some(GitViewSearch {
            matches: (0..30)
                .map(|i| GitViewSearchMatch {
                    path: format!("src/f{:02}.rs", i % 3),
                    line: (i + 1) as u64,
                    text: format!("hit {i}"),
                })
                .collect(),
            truncated: false,
        });
        let mut rt = section_runtime(s);

        assert!(keyed(&rt, nav_scroll_scope(Section::Files)).is_some());
        let first = search_row_key("src/f00.rs", 1);
        assert!(keyed(&rt, &first).is_some(), "row key mounted");

        press_j(&mut rt, 1);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 0);
        let row = keyed(&rt, &first).expect("nav row mounted");
        assert_eq!(row.fill, rt.state.ink.accent.alpha(0.14));

        press_j(&mut rt, 29);
        let nav = rt.state.editor.keyboard.nav.clone().unwrap();
        assert_eq!(nav.index, 29);
        // items[29] = match i=29 → path src/f(29%3=2).rs → f02.rs line 30
        let last_key = search_row_key("src/f02.rs", 30);
        let row = keyed(&rt, &last_key).expect("last row mounted");
        assert!(
            row.visible[3] > row.rect[3] * 0.5,
            "scrolled into view: visible={:?} rect={:?}",
            row.visible,
            row.rect
        );

        rt.state.editor.doc_load = None;
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Default::default(),
        });
        rt.frame(0.0);
        assert!(
            rt.state.editor.doc_load.is_some() || rt.state.editor.doc.is_some(),
            "enter queued the open through open_doc"
        );
    }
}

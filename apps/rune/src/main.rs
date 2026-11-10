//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls, a terminal stub and the
//! native editor workspace backed by `app-core` (real projects, Git sections,
//! drafts with Save/Discard/Cancel guards).

mod chrome;
mod data;
mod editor;
mod sidebar;
mod term;
mod theme;

use std::collections::HashMap;
use std::sync::OnceLock;

use rune::{AppSpec, Cx, El, Tx, col, row};

use crate::data::Data;
use crate::editor::backend;
use crate::theme::Ink;

/// The workspace column's content mode: the terminal stub, the editor
/// workspace, or the app-level settings page that replaces the whole column
/// — TerminalWorkspace's view switch plus App.tsx's `settingsMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Terminal,
    Editor,
    Settings,
}

pub struct State {
    ink: Ink,
    data: Data,
    lines: Vec<term::TermLine>,
    /// Project id → expanded; absent means expanded (ProjectTree default).
    expanded: HashMap<String, bool>,
    selected_session: Option<String>,
    active_workspace: Option<String>,
    /// Open workspace tabs (titles); seeded from `Data::tabs`.
    open_tabs: Vec<String>,
    active_tab: usize,
    /// "Worktrees" group header open per project id; absent means open.
    worktree_groups: HashMap<String, bool>,
    /// Per-worktree session list open, keyed by worktree path; absent = open.
    worktrees_open: HashMap<String, bool>,
    /// Workspace column mode (terminal / editor / settings).
    view: View,
    /// The native editor workspace: projects/checkouts, sections, docs,
    /// backend worker and ACP runtime.
    editor: editor::state::EditorState,
}

/// `AppSpec::init` is a plain `fn() -> S`, so the `AppHandle` the editor's
/// backend/ACP threads need is parked here between `App::new` and `run`
/// (init runs inside `run`, after the handle exists).
static APP_HANDLE: OnceLock<rune::AppHandle<State>> = OnceLock::new();

fn init_state() -> State {
    let handle = APP_HANDLE
        .get()
        .cloned()
        .expect("app handle registered before run");
    State::new(handle)
}

impl State {
    fn new(handle: rune::AppHandle<State>) -> State {
        let ink = Ink::new();
        let data = data::mock_data();
        let lines = term::real_term_lines(&ink);
        let open_tabs = data.tabs.clone();
        let active_tab = data.active_tab;
        let kit = rune_kit::Kit::neutral(true);
        // Workers own their storage connections; nothing here touches
        // GOKO_HOME on the UI thread.
        let editor = editor::state::EditorState::new(kit, handle);
        State {
            ink,
            data,
            lines,
            expanded: HashMap::new(),
            selected_session: Some("sess-devin-spike".to_string()),
            // No workspace is committed until the user picks a real
            // registered project/checkout in the sidebar (or the editor
            // pickers) — the mock data layer must never seed editor
            // identity.
            active_workspace: None,
            open_tabs,
            active_tab,
            worktree_groups: HashMap::new(),
            worktrees_open: HashMap::new(),
            view: View::Terminal,
            editor,
        }
    }
}

/// Centered title + muted body for views this milestone doesn't implement —
/// keeps the chrome's buttons honest instead of dead.
fn placeholder(title: &str, body: &str, ink: &Ink) -> El {
    col()
        .grow(1.0)
        .w_full()
        .h_full()
        .items_center()
        .justify_center()
        .gap(8.0)
        .child(theme::txt(title, theme::T14M, ink.text))
        .child(theme::txt(body, theme::T12, ink.muted))
}

/// A handle for state-level tests: `App::new` builds the shared request
/// queue without opening a window, so `update`/`quit` calls from worker
/// replies just queue harmlessly.
#[cfg(test)]
pub(crate) fn test_handle() -> rune::AppHandle<State> {
    fn init() -> State {
        unreachable!("init does not run in tests")
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        unreachable!("view does not run in tests")
    }
    rune::App::new(AppSpec::new("test", (10.0, 10.0), init, view)).handle()
}

/// Root: header full-width, then [sidebar 320 | 8 px separator | workspace
/// column]. Settings replaces the whole column like App.tsx's settingsMode;
/// Editor/Terminal keep the tab strip + controls and swap only the content.
fn view(s: &State, cx: &mut Cx<State>) -> El {
    let ink = &s.ink;
    let workspace = if s.view == View::Settings {
        col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(placeholder(
                "Settings",
                "The settings page lands in a later milestone.",
                ink,
            ))
    } else {
        let content = if s.view == View::Editor {
            editor::workspace::workspace(s, ink, &s.editor.kit)
        } else {
            term::terminal_area(s, ink)
        };
        // SessionControls live inside TerminalWorkspace only — the editor
        // has no session pane, so the strip (and its mock Stop/Close) stays
        // terminal-only like App.tsx.
        let mut stack = col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(chrome::tab_bar(s, ink));
        if s.view != View::Editor {
            stack = stack.child(chrome::controls_bar(s, ink));
        }
        stack.child(content)
    };
    let mut root = col()
        .w_full()
        .h_full()
        .fill(ink.bg)
        .child(chrome::header(s, ink))
        .child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(sidebar::sidebar(s, ink))
                .child(sidebar::separator(ink))
                .child(workspace),
        );
    // Caret-anchored LSP hover card: mounts only once `Cx::caret` has
    // measured the field's caret in window points.
    if s.view == View::Editor
        && let Some(layer) = editor::lsp::hover_layer(s, cx, ink, &s.editor.kit)
    {
        root = root.child(layer);
    }
    // Quick-open palette: scrim + top card mount above everything, the same
    // pair `rune_kit::Dialog` pushes for its modals.
    if s.view == View::Editor && s.editor.quick_open.open {
        for l in editor::quick_open::layers(s, ink, &s.editor.kit) {
            root = root.child(l);
        }
    }
    root
}

/// `AppSpec::setup`: kick the initial project list, history-layout restore,
/// and ACP agent discovery once the state exists.
fn setup(s: &mut State, _waker: rune::Waker) {
    s.editor.refresh_projects();
    // History pane sizes persist in the shared settings store; the read runs
    // on the worker like every storage access (no UI-thread SQLite).
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::history_layout::HISTORY_LAYOUT_KEY.to_string(),
        }),
    );
    s.editor.acp.refresh();
}

/// `AppSpec::poll`: drain editor housekeeping — quit-guard swizzle install,
/// cancelled-close reopen, search debounce, save-resume navigation, queued
/// external opens, and pending line reveals.
fn poll(s: &mut State, _now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    editor::quit_guard::poll_install();
    editor::navigation::reopen_if_cancelled(s);
    changed |= editor::search::poll(s);
    if let Some(pending) = s.editor.pending_apply.take() {
        // Route through the guard: a save that landed clean applies
        // immediately; anything still dirty re-raises the dialog.
        editor::navigation::navigate(s, pending, tx);
        changed = true;
    }
    editor::highlight::poll(&mut s.editor, tx);
    // LSP: drain runtime messages, reconcile document sync, drive hover.
    changed |= editor::lsp::poll(&mut s.editor, tx);
    // Quick-open: rebuild matches when the path cache or checkout moved.
    changed |= editor::quick_open::poll(s);
    // A resolved LSP definition navigates through the same dirty-draft
    // guard as every other pending navigation.
    if let Some(nav) = editor::lsp::take_nav(&mut s.editor) {
        editor::navigation::navigate(s, nav, tx);
        changed = true;
    }
    // An empty/failed LSP definition reply falls back to the heuristic
    // go-to pipeline.
    if let Some(req) = editor::lsp::take_fallback(&mut s.editor) {
        editor::navigation::start_go_to_heuristic(s, req, tx);
        changed = true;
    }
    // History housekeeping that must run even while its rect is unchanged:
    // auto-select, jump_to consumption, table↔layout sync, detail fetches.
    changed |= editor::history::poll(s, tx);
    s.editor.poll_layout_save();
    if let Some(doc) = s.editor.doc.as_mut()
        && let Some(line) = doc.reveal_line.take()
    {
        // CodeState::reveal selects the line, focuses the field and
        // centres it in the scroll viewport (1-based, clamped).
        doc.code.reveal(line as u32, tx);
        changed = true;
    }
    changed
}

/// Window close (red button / Cmd-W): a dirty draft cancels the close —
/// Rune's close is a notification, so `keep_running` + reopen carries the
/// guard instead of a veto.
fn on_close(s: &mut State, tx: &mut Tx) {
    editor::navigation::on_window_close(s, tx);
}

/// Real quit (post-veto or clean): shut the backend and ACP runtimes down
/// with the app — `Backend::shutdown` refuses new jobs, lets accepted
/// mutating jobs finish, then joins; `Acp::shutdown` joins its runtime
/// thread after `hub.shutdown()`.
fn on_quit(s: &mut State, _tx: &mut Tx) {
    s.editor.backend.shutdown();
    s.editor.acp.shutdown();
    s.editor.lsp.shutdown();
    s.editor.sync_quit_dirty();
}

/// Unhandled keys: the editor's global chords and section/list shortcuts
/// first (⌘S/⌘F/⌘⇧F/⌘P, 1–4, j/k, Enter — `editor::keyboard`), the quick-open
/// palette owns every key while open, then F12 runs go-to-definition at the
/// caret (LSP first, heuristic fallback) and Esc dismisses the hover card.
fn on_key(s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if s.view != View::Editor {
        return false;
    }
    if s.editor.quick_open.open {
        return editor::quick_open::key(s, ev, tx);
    }
    if editor::keyboard::on_key(s, ev, tx) {
        return true;
    }
    match ev.key {
        rune::Key::Function(12) => {
            // Go-to-definition belongs to the Files code surface: require
            // the section AND a focused editable field so a stray F12 in
            // History/Changes can't navigate on a stale caret.
            if s.editor.section != editor::state::Section::Files {
                return false;
            }
            let Some(doc) = s.editor.doc.as_ref() else {
                return false;
            };
            if !doc.editable() || !doc.code.edit.focused {
                return false;
            }
            let Some(req) = doc.code.go_to_token_at(doc.code.edit.focus) else {
                return false;
            };
            editor::navigation::start_go_to(s, req, tx);
            true
        }
        rune::Key::Escape => s.editor.lsp_dismiss_hover(),
        _ => false,
    }
}

/// Modifier snapshot for the code owner's glyph-hit link overlays —
/// `EditorState::set_modifiers` mirrors the platform go-to modifier onto the
/// active document's `CodeState`.
fn on_modifiers(s: &mut State, mods: rune::Mods, _tx: &mut Tx) {
    s.editor.set_modifiers(mods);
}

fn main() {
    let app = rune::App::new(AppSpec {
        families: ("Inter", "ui-monospace"),
        min_size: Some((760.0, 480.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Goko", (1512.0, 814.0), init_state, view)
    })
    .keep_running()
    .on_close(on_close)
    .on_quit(on_quit)
    .on_modifiers(on_modifiers);
    APP_HANDLE
        .set(app.handle())
        .unwrap_or_else(|_| unreachable!("app handle registered once"));
    app.run();
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::navigation::{Guard, GuardAction, PendingNav};
    use crate::editor::state::Section;
    use app_core::git_view::{GitViewFile, GitViewSearch, GitViewSearchMatch};
    use app_core::projects::ProjectView;
    use app_core::store::Project;
    use app_core::worktrees::{WorktreeInfo, WorktreeList};
    use rune::{Env, Key, KeyEvent, Mods, Runtime};
    use std::path::PathBuf;

    fn project(id: &str, name: &str, root: &str) -> ProjectView {
        ProjectView {
            project: Project {
                id: id.into(),
                name: name.into(),
                root: root.into(),
                created_at: 0,
            },
            missing: false,
        }
    }

    fn wt(name: &str, path: &str, is_root: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.into(),
            path: path.into(),
            branch: None,
            head: String::new(),
            is_root,
            missing: false,
            dirty: None,
            changed: None,
        }
    }

    /// Full-app Editor scene at `view == View::Editor`: registered project,
    /// selected checkout, populated file tree — and the mock terminal
    /// session still selected, so a leaked controls bar would show.
    fn editor_scene() -> State {
        let mut s = State::new(test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        s.editor.projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ];
        s.editor.worktrees = WorktreeList {
            repo: true,
            worktrees: vec![wt("web", "/work/a/web", true)],
        };
        let co = PathBuf::from("/tmp/goko-view-qa");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.section = Section::Files;
        s.editor.files.paths = [
            "README.md",
            "TREE_ONLY.txt",
            "docs/guide.md",
            "src/a.rs",
            "src/b.rs",
            "top.txt",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        s.editor.rebuild_file_model();
        s
    }

    /// An editable text doc whose buffer differs from the saved text.
    fn dirty_doc() -> OpenDoc {
        let file = GitViewFile {
            path: "src/a.rs".into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        let mut doc = OpenDoc::new("src/a.rs", &file);
        doc.code.edit.text.push_str("// unsaved edit\n");
        assert!(doc.dirty());
        doc
    }

    fn dirty_scene() -> State {
        let mut s = editor_scene();
        s.editor.doc = Some(dirty_doc());
        s
    }

    fn els(rt: &Runtime<State>) -> Vec<rune::ElementInfo> {
        rt.elements()
    }

    fn press(rt: &mut Runtime<State>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    /// Index of the element whose ancestor chain (or itself) has `role`.
    fn inside_role(els: &[rune::ElementInfo], idx: usize, role: &str) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            if els[n].role == role {
                return true;
            }
            i = els[n].parent;
        }
        false
    }

    /// Every mounted element stays inside the viewport horizontally —
    /// the regression the fixed-width columns caused.
    fn assert_no_horizontal_overflow(rt: &Runtime<State>, width: f32) {
        let mut bad = Vec::new();
        for e in els(rt) {
            let v = e.visible;
            if v[2] > 0.5 && !(v[0] >= -0.5 && v[0] + v[2] <= width + 0.5) {
                bad.push(format!(
                    "role={:?} label={:?} text={:?} key={:?} visible={:?}",
                    e.role,
                    e.label,
                    e.text,
                    e.key.as_deref(),
                    v
                ));
            }
        }
        assert!(
            bad.is_empty(),
            "overflow in {}px:\n{}",
            width,
            bad.join("\n")
        );
    }

    #[test]
    fn editor_view_hides_terminal_session_controls() {
        // State::new keeps `selected_session` + mock sessions: before the
        // fix the controls bar rendered Stop/Close pane inside the editor.
        let s = editor_scene();
        assert!(s.selected_session.is_some());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for e in els(&rt) {
            for text in [e.label.as_deref(), e.text.as_deref()]
                .into_iter()
                .flatten()
            {
                assert_ne!(text, "Stop", "session control leaked into editor");
                assert_ne!(text, "Close pane", "session control leaked into editor");
            }
        }
    }

    #[test]
    fn pickers_fit_duplicate_project_names_at_760() {
        let mut s = editor_scene();
        // Two registered projects sharing a name with deep canonical roots:
        // labels disambiguate by path and must not stretch the bar.
        s.editor.projects = vec![
            project(
                "p1",
                "web",
                "/Users/dev/work/clients/acme-corp/frontend/apps/web",
            ),
            project(
                "p2",
                "web",
                "/Users/dev/work/clients/globex-inc/backend/services/web",
            ),
        ];
        s.editor.project_id = Some("p1".into());
        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let mut combos: Vec<_> = els(&rt)
            .into_iter()
            .filter(|e| e.role == "combobox")
            .collect();
        combos.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        assert_eq!(combos.len(), 2, "project + checkout pickers mounted");
        for c in &combos {
            assert!(c.rect[2] > 40.0, "picker collapsed: {:?}", c.rect);
            assert!(
                c.rect[0] >= 0.0 && c.rect[0] + c.rect[2] <= 760.0,
                "picker escapes viewport: {:?}",
                c.rect
            );
        }
        // The project picker's help keeps the full disambiguated identity.
        let help = combos[0].help.as_deref().unwrap_or_default();
        assert!(
            help.contains("acme-corp"),
            "project picker lost its path identity: {help:?}"
        );
        let add = els(&rt)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Add project"))
            .expect("Add project button mounted");
        assert!(
            add.rect[0] + add.rect[2] <= 760.0 && add.rect[2] > 0.0,
            "Add project escapes viewport: {:?}",
            add.rect
        );
        assert_no_horizontal_overflow(&rt, 760.0);
    }

    #[test]
    fn search_open_shows_results_in_aside_without_overflow() {
        let mut s = dirty_scene();
        s.editor.doc = Some({
            let file = GitViewFile {
                path: "src/a.rs".into(),
                kind: "text",
                text: Some("fn main() {}\n".into()),
                truncated: false,
                size: 12,
            };
            OpenDoc::new("src/a.rs", &file)
        });
        s.editor.files.search.open = true;
        s.editor.files.search.results = Some(GitViewSearch {
            matches: (1..=30)
                .map(|i| GitViewSearchMatch {
                    path: format!("src/f{i}.rs"),
                    line: i,
                    text: "fn f() -> i32 { i }".into(),
                })
                .collect(),
            truncated: false,
        });
        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let e = els(&rt);
        // Results live in the aside (desktop SplitPane parity): a match row
        // is mounted and the file tree is gone.
        assert!(
            e.iter()
                .any(|el| el.label.as_deref() == Some("src/f1.rs:1")),
            "search result row missing"
        );
        assert!(
            !e.iter()
                .any(|el| el.label.as_deref() == Some("TREE_ONLY.txt")),
            "file tree still mounted beside results"
        );
        assert_no_horizontal_overflow(&rt, 760.0);
        // The editor surface keeps usable width in the remaining space:
        // its `text_input` mounts below the toolbars (y > 150 at this
        // viewport) and must be wider than a sliver.
        let editor_fields: Vec<_> = els(&rt)
            .into_iter()
            .filter(|el| el.role == "textbox" && el.rect[1] > 150.0)
            .collect();
        assert!(
            editor_fields.iter().any(|el| el.rect[2] > 100.0),
            "editor surface collapsed: {editor_fields:?}"
        );
    }

    #[test]
    fn guard_dialog_is_modal_with_inert_background() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let e = els(&rt);
        let dialog_idx = e
            .iter()
            .position(|el| el.role == "dialog")
            .expect("guard dialog mounted with dialog role");
        // Background controls are inert; the dialog's own controls are not.
        let tab_idx = e
            .iter()
            .position(|el| el.role == "tab")
            .expect("section tabs mounted");
        assert!(e[tab_idx].inert, "background tab not inert under modal");
        for label in ["Cancel", "Discard", "Save"] {
            // The doc bar also renders Save/Discard for the dirty draft —
            // pick the control INSIDE the dialog.
            let idx = e
                .iter()
                .enumerate()
                .find(|(i, el)| {
                    (el.label.as_deref() == Some(label) || el.text.as_deref() == Some(label))
                        && inside_role(&e, *i, "dialog")
                })
                .map(|(i, _)| i)
                .unwrap_or_else(|| panic!("{label} button mounted in dialog"));
            assert!(!e[idx].inert, "{label} inert inside dialog");
        }
        let _ = dialog_idx;
    }

    #[test]
    fn escape_cancels_guard_keeping_doc_identity_and_text() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(rt.state.editor.guard.is_some());

        press(&mut rt, Key::Escape);
        assert!(rt.state.editor.guard.is_none(), "Escape dismissed guard");
        let doc = rt.state.editor.doc.as_ref().expect("doc retained");
        assert!(doc.dirty(), "draft must stay dirty after Cancel");
        assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
        assert_eq!(doc.path, "src/a.rs");
        // Identity + pending navigation all preserved: Cancel drops the
        // pending SetSection, keeps project/checkout/section as they were.
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout.as_deref(),
            Some(std::path::Path::new("/tmp/goko-view-qa"))
        );
        assert_eq!(rt.state.editor.section, Section::Files);
    }

    #[test]
    fn tab_focus_stays_trapped_inside_guard_dialog() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);

        // Walk Tab around the dialog; focus must never land on inert
        // background chrome.
        for _ in 0..8 {
            press(&mut rt, Key::Tab);
            let e = els(&rt);
            if let Some(fidx) = e.iter().position(|el| el.focused) {
                assert!(
                    inside_role(&e, fidx, "dialog"),
                    "Tab escaped the guard dialog to {:?}",
                    e[fidx].label
                );
            }
        }
    }

    #[test]
    fn section_tabs_report_selected_state() {
        let s = editor_scene();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // The workspace tab strip also uses role="tab" — section tabs are
        // keyed `section:<label>`.
        let tabs: Vec<_> = els(&rt)
            .into_iter()
            .filter(|e| {
                e.role == "tab" && e.key.as_deref().is_some_and(|k| k.starts_with("section:"))
            })
            .collect();
        assert_eq!(tabs.len(), 5, "five section tabs mounted");
        let selected: Vec<_> = tabs.iter().filter(|t| t.item_selected).collect();
        assert_eq!(selected.len(), 1, "exactly one tab selected");
        assert_eq!(selected[0].label.as_deref(), Some("Files"));
        for t in &tabs {
            assert_eq!(
                t.item_selected,
                t.label.as_deref() == Some("Files"),
                "tab {:?} selected state wrong",
                t.label
            );
        }
    }

    /// Doc bar at the narrow 760px viewport with a long path, a dirty
    /// draft (Save/Discard live), staged change data (Show changes) and a
    /// save error (Reload from disk): every control stays mounted inside
    /// the window — the path label truncates instead of pushing buttons
    /// off-screen.
    #[test]
    fn doc_bar_controls_fit_long_path_at_760() {
        let mut s = editor_scene();
        let long_path = "src/deeply/nested/directory/structure/with/a/very/long/file/name.rs";
        let file = GitViewFile {
            path: long_path.into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        let mut doc = OpenDoc::new(long_path, &file);
        doc.code.edit.text.push_str("// unsaved edit\n");
        doc.save_error = Some("disk conflict".into());
        s.editor.doc = Some(doc);
        s.editor.changes.data = Some(app_core::git_view::GitViewChanges {
            branch: Some("main".into()),
            head: "abc1234".into(),
            conflicts: vec![],
            staged: vec![app_core::git_view::StatusEntry {
                path: long_path.into(),
                orig_path: None,
                status: "modified",
                added: None,
                removed: None,
            }],
            unstaged: vec![],
            untracked: vec![],
            ahead: None,
        });
        s.editor.changes.checkout = s.editor.checkout_key.clone();

        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let labels: Vec<String> = els(&rt).into_iter().filter_map(|e| e.label).collect();
        for want in ["Save", "Discard", "Show changes", "Reload from disk"] {
            assert!(
                labels.iter().any(|l| l == want),
                "missing {want}: {labels:?}"
            );
        }
        assert_no_horizontal_overflow(&rt, 760.0);
    }

    // -- sidebar → editor identity -------------------------------------------

    /// Two registered projects as the backend's Projects reply leaves them,
    /// each with its own loaded worktree list — p1 has two linked
    /// worktrees, p2 has one. Terminal view, nothing committed yet.
    fn sidebar_scene() -> State {
        let mut s = State::new(test_handle());
        s.editor.projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ];
        let lists = [
            (
                "p1",
                vec![
                    wt("web", "/work/a/web", true),
                    wt("feat-a", "/work/a/feat-a", false),
                    wt("feat-b", "/work/a/feat-b", false),
                ],
            ),
            (
                "p2",
                vec![
                    wt("api", "/work/b/api", true),
                    wt("fix", "/work/b/fix", false),
                ],
            ),
        ];
        for (pid, worktrees) in lists {
            s.editor.sidebar_worktrees.insert(
                pid.to_string(),
                crate::editor::state::SidebarWorktrees {
                    list: Some(WorktreeList {
                        repo: true,
                        worktrees,
                    }),
                    loading: false,
                    error: None,
                },
            );
        }
        s
    }

    /// Editor already committed to p1's root checkout — the "already in
    /// Editor" starting point for in-editor sidebar switches.
    fn sidebar_editor_scene() -> State {
        let mut s = sidebar_scene();
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/work/a/web"),
        ));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        s
    }

    fn click_key(rt: &mut Runtime<State>, key: &str) {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("element {key} mounted"));
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn sidebar_lists_real_projects_and_mock_sessions_stay_unassigned() {
        let s = sidebar_scene();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = els(&rt);
        for key in [
            "sidebar-project:p1",
            "sidebar-project:p2",
            "sidebar-worktree:p1:0",
            "sidebar-worktree:p1:1",
            "sidebar-worktree:p2:0",
            "sidebar-unassigned",
        ] {
            assert!(
                e.iter().any(|el| el.key.as_deref() == Some(key)),
                "missing {key}"
            );
        }
        // Mock project ids never become sidebar rows — they are not
        // registered projects, so nothing may select them as identity.
        for mock in ["proj-goko", "proj-notes"] {
            assert!(
                !e.iter()
                    .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{mock}"))),
                "mock project {mock} leaked into the sidebar"
            );
        }
        // Mock session rows render under Unassigned, never nested under a
        // real project — the honest terminal/session boundary.
        let texts: Vec<&str> = e
            .iter()
            .filter_map(|el| el.text.as_deref().or(el.label.as_deref()))
            .collect();
        assert!(texts.iter().any(|t| t.contains("fix-auth-flow")));
        assert!(texts.contains(&"Unassigned"));
    }

    #[test]
    fn sidebar_project_row_activates_real_checkout_then_editor_opens_it() {
        let s = sidebar_scene();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // The original bug: pick a real project in the sidebar, then open
        // the Editor — the SAME project+checkout must activate.
        click_key(&mut rt, "sidebar-project:p1");
        assert_eq!(rt.state.view, View::Terminal, "selection alone stays put");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/web")),
            "root row selects the registered project root"
        );
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/web")),
            "Editor activated the sidebar's committed checkout"
        );
    }

    #[test]
    fn sidebar_worktree_row_selects_exact_checkout_while_in_editor() {
        let mut rt = Runtime::new(
            sidebar_editor_scene(),
            view,
            (1100.0, 700.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        // Already in Editor on p1's root: a worktree row press must move
        // project+checkout identity to that exact registered path.
        click_key(&mut rt, "sidebar-worktree:p1:1"); // feat-b
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/feat-b"))
        );
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/feat-b").as_str())
        );
        // A different project's worktree row switches project AND checkout.
        click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/b/fix")));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/fix").as_str())
        );
    }

    #[test]
    fn sidebar_select_with_dirty_doc_guard_discard_commits() {
        let mut s = sidebar_editor_scene();
        s.editor.doc = Some(dirty_doc());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-project:p2");
        // The guard held: NOTHING moved — project, checkout, workspace
        // highlight and the dirty draft all still belong to p1.
        assert!(rt.state.editor.guard.is_some());
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        assert!(rt.state.editor.doc.as_ref().unwrap().dirty());
        // Resolve the guard the way the dialog's Discard button does —
        // Runtime's headless hit-test does not deliver presses inside
        // modal layers, so the state-level path is the honest check.
        rt.update(|s, tx| {
            crate::editor::navigation::resolve_guard(s, GuardAction::Discard, tx);
        });
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/b/api")),
            "Discard applied the pending sidebar selection"
        );
        assert!(rt.state.editor.doc.is_none());
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
        );
    }

    #[test]
    fn sidebar_select_with_dirty_doc_guard_cancel_restores_everything() {
        let mut s = sidebar_editor_scene();
        s.editor.doc = Some(dirty_doc());
        let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
        assert!(rt.state.editor.guard.is_some());
        rt.update(|s, tx| {
            crate::editor::navigation::resolve_guard(s, GuardAction::Cancel, tx);
        });
        assert!(rt.state.editor.guard.is_none(), "Cancel resolved the guard");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/a/web")));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str()),
            "Cancel restored the previous visible workspace identity"
        );
        let doc = rt.state.editor.doc.as_ref().expect("draft retained");
        assert!(doc.dirty());
        assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
    }

    #[test]
    fn sidebar_status_rows_cover_loading_error_and_empty() {
        // Fresh State::new: no projects yet, no load in flight — the honest
        // empty state, never a fabricated row.
        let s = State::new(test_handle());
        assert!(s.active_workspace.is_none());
        assert!(s.editor.project_id.is_none());
        assert!(s.editor.checkout.is_none());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt)
            .iter()
            .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("No projects registered")),
            "empty state missing: {texts:?}"
        );

        // Initial async load in flight → loading row. Mutations go through
        // `update` so the next frame knows the view reads changed state.
        rt.update(|s, _| s.editor.projects_loading = true);
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt)
            .iter()
            .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
            .collect();
        assert!(texts.iter().any(|t| t.contains("Loading projects")));

        // Registry error with no projects → the error itself, in danger tone.
        rt.update(|s, _| {
            s.editor.projects_loading = false;
            s.editor.projects_error = Some("goko.db is locked".into());
        });
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains("goko.db is locked")));

        // A per-project worktree failure renders under ITS row only.
        rt.update(|s, _| *s = sidebar_scene());
        rt.update(|s, _| {
            let entry = s
                .editor
                .sidebar_worktrees
                .get_mut("p2")
                .expect("p2 worktrees seeded");
            entry.error = Some("not a git repository".into());
            entry.list = None;
        });
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains("not a git repository")));
        // p1's rows are untouched.
        assert!(
            els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("sidebar-worktree:p1:0"))
        );
    }

    /// The real end-to-end fixture: two registered projects in a temp
    /// `Store`, real `git worktree` checkouts, real `worktrees::list`
    /// payloads — driven through the same sidebar → navigation → editor
    /// identity path production uses.
    #[test]
    fn sidebar_drives_editor_with_real_registered_projects_and_worktrees() {
        use std::process::Command;

        fn git_ok(root: &std::path::Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git run");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        fn repo(base: &std::path::Path, name: &str) -> std::path::PathBuf {
            let root = base.join(name);
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            git_ok(&root, &["init", "-b", "main"]);
            git_ok(&root, &["config", "user.name", "Tester"]);
            git_ok(&root, &["config", "user.email", "tester@example.com"]);
            std::fs::write(root.join("file.txt"), "x\n").unwrap();
            git_ok(&root, &["add", "-A"]);
            git_ok(&root, &["commit", "-qm", "init"]);
            root
        }

        let home = app_core::test_support::TempDir::new("rune-sidebar");
        let base = home.path().canonicalize().unwrap();
        let root_a = repo(&base, "proj-web");
        let root_b = repo(&base, "proj-api");
        // Two linked worktrees on proj-web, one on proj-api.
        git_ok(&root_a, &["worktree", "add", "../wt-a", "-b", "feat-a"]);
        git_ok(&root_a, &["worktree", "add", "../wt-b", "-b", "feat-b"]);
        git_ok(&root_b, &["worktree", "add", "../wt-c", "-b", "fix-c"]);

        // Register through the real store — the same rows Req::Projects
        // and Req::Worktrees return in production.
        let store = app_core::store::Store::open(home.path()).unwrap();
        let pv_a = app_core::projects::add_project(&store, root_a.to_str().unwrap()).unwrap();
        let pv_b = app_core::projects::add_project(&store, root_b.to_str().unwrap()).unwrap();
        let list_a = app_core::worktrees::list(&root_a).unwrap();
        let list_b = app_core::worktrees::list(&root_b).unwrap();
        assert_eq!(list_a.worktrees.len(), 3, "root + two linked on web");
        assert_eq!(list_b.worktrees.len(), 2, "root + one linked on api");

        let id_a = pv_a.project.id.clone();
        let id_b = pv_b.project.id.clone();
        let mut s = State::new(test_handle());
        s.editor.projects = vec![pv_a, pv_b];
        for (pid, list) in [(&id_a, list_a), (&id_b, list_b)] {
            s.editor.sidebar_worktrees.insert(
                pid.clone(),
                crate::editor::state::SidebarWorktrees {
                    list: Some(list),
                    loading: false,
                    error: None,
                },
            );
        }
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // Real project names render as rows keyed by their real ids.
        let e = els(&rt);
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_a}")))
        );
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_b}")))
        );

        // Root row → the registered canonical root, exactly.
        click_key(&mut rt, &format!("sidebar-project:{id_a}"));
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_a.as_str()));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from(&root_a)),
            "root row committed the canonical registered root"
        );

        // Worktree row → the exact path `git worktree list` reported.
        let wt_path = rt.state.editor.sidebar_worktrees[&id_a]
            .list
            .as_ref()
            .unwrap()
            .worktrees
            .iter()
            .find(|w| !w.is_root)
            .unwrap()
            .path
            .clone();
        click_key(&mut rt, &format!("sidebar-worktree:{id_a}:0"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from(&wt_path)),
            "worktree row committed the exact listed checkout"
        );

        // Editor toggle keeps the committed identity; switching projects
        // through the sidebar while in Editor moves it wholesale.
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&wt_path)));
        click_key(&mut rt, &format!("sidebar-project:{id_b}"));
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
        click_key(&mut rt, "view:Terminal");
        assert_eq!(rt.state.view, View::Terminal);
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
    }

    /// Drain the channel-backed backend exactly like the worker thread:
    /// every accepted job runs through the real `run_job` against the real
    /// temp `Storage`, and its reply applies in FIFO order — including any
    /// jobs a reply itself enqueues (the Projects ensure pass, worktree
    /// re-reads, section refreshes).
    fn drain_backend(
        s: &mut State,
        rx: &std::sync::mpsc::Receiver<crate::editor::backend::Job>,
        storage: &Option<app_core::store::Storage>,
    ) {
        while let Ok(job) = rx.try_recv() {
            let reply = crate::editor::backend::run_job(storage, &job);
            s.editor.apply_reply(reply);
            if s.editor.section == Section::Changes {
                crate::editor::changes::ensure_selection(s);
            }
        }
    }

    /// Regression for the native QA hang: two registered projects sharing
    /// the display name "repo" whose stored roots are NON-canonical
    /// (`/var/folders/…`) while `git worktree list` reports canonical
    /// (`/private/var/folders/…`) paths — the exact DB-vs-Git divergence the
    /// QA fixture had. Selecting project B's root and then project A's
    /// worktree must commit the exact checkout and let Changes settle,
    /// driven through the real `run_job`/`apply_reply` stream rather than
    /// synthetic replies.
    #[test]
    fn cross_project_worktree_selection_settles_with_real_backend_replies() {
        use std::process::Command;

        fn git_ok(root: &std::path::Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git run");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        fn repo(base: &std::path::Path, rel: &str) -> PathBuf {
            let root = base.join(rel);
            std::fs::create_dir_all(&root).unwrap();
            git_ok(&root, &["init", "-b", "main"]);
            git_ok(&root, &["config", "user.name", "Tester"]);
            git_ok(&root, &["config", "user.email", "tester@example.com"]);
            std::fs::write(root.join("file.txt"), "x\n").unwrap();
            git_ok(&root, &["add", "-A"]);
            git_ok(&root, &["commit", "-qm", "init"]);
            root // deliberately NOT canonicalized: the QA DB kept /var/…
        }

        let home = app_core::test_support::TempDir::new("rune-qa-repro");
        let base = home.path().join("repos");
        std::fs::create_dir_all(&base).unwrap();
        let root_a = repo(&base, "repo");
        let root_b = repo(&base, "other/repo");
        git_ok(
            &root_a,
            &[
                "worktree",
                "add",
                &base.join("wt-a").display().to_string(),
                "-b",
                "wt-a",
            ],
        );
        git_ok(
            &root_a,
            &[
                "worktree",
                "add",
                &base.join("nested/wt-a2").display().to_string(),
                "-b",
                "wt-a2",
            ],
        );
        git_ok(
            &root_b,
            &[
                "worktree",
                "add",
                &base.join("other/wt-b").display().to_string(),
                "-b",
                "wt-b",
            ],
        );

        // Register VERBATIM non-canonical roots under the same display
        // name — `Store::add_project` stores the string it is given, which
        // is how the QA database ended up with /var/… roots.
        let storage = app_core::store::Storage::open(home.path());
        let store = storage.get().unwrap();
        let pa = store.add_project("repo", root_a.to_str().unwrap()).unwrap();
        let pb = store.add_project("repo", root_b.to_str().unwrap()).unwrap();
        assert_ne!(pa.id, pb.id);
        let storage = Some(storage);

        let (backend, rx) = crate::editor::backend::channel_backend();
        let mut s = State::new(test_handle());
        s.editor.backend = backend;
        s.view = View::Editor;

        // Startup project load + the per-project worktree ensure pass.
        s.editor.refresh_projects();
        drain_backend(&mut s, &rx, &storage);
        assert_eq!(s.editor.projects.len(), 2);
        assert!(
            s.editor
                .sidebar_worktrees
                .get(&pa.id)
                .and_then(|e| e.list.as_ref())
                .is_some_and(|l| l.worktrees.iter().any(|w| !w.is_root))
        );

        // The worktree row's checkout is the path `git worktree list`
        // reported, verbatim — exactly what sidebar.rs sends.
        let wt_path = s.editor.sidebar_worktrees[&pa.id]
            .list
            .as_ref()
            .unwrap()
            .worktrees
            .iter()
            .find(|w| !w.is_root && w.path.ends_with("wt-a"))
            .expect("linked worktree listed")
            .path
            .clone();

        // QA step A: sidebar project-B root row while in Terminal→Editor.
        crate::editor::navigation::navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: pb.id.clone(),
                checkout: PathBuf::from(&pb.root),
            },
            &mut rune::Tx::default(),
        );
        drain_backend(&mut s, &rx, &storage);
        assert_eq!(s.editor.project_id.as_deref(), Some(pb.id.as_str()));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new(&pb.root)),
        );
        assert!(
            s.editor.changes.data.is_some(),
            "root checkout changes loaded"
        );
        assert!(!s.editor.changes.loading);

        // QA step B: while in Editor, the sidebar worktree row of project A.
        crate::editor::navigation::navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: pa.id.clone(),
                checkout: PathBuf::from(&wt_path),
            },
            &mut rune::Tx::default(),
        );
        drain_backend(&mut s, &rx, &storage);

        // The committed identity is the exact listed checkout…
        assert_eq!(s.editor.project_id.as_deref(), Some(pa.id.as_str()));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new(&wt_path)),
            "worktree selection committed the exact listed path"
        );
        // …the picker has real options for it (not stuck on the placeholder)…
        assert!(
            !s.editor.worktrees.worktrees.is_empty(),
            "checkout picker populated for the selected project"
        );
        assert!(!s.editor.worktrees_loading);
        assert!(s.editor.worktrees_error.is_none());
        // …and Changes settled on the new checkout instead of spinning.
        assert!(
            s.editor.changes.data.is_some(),
            "changes loaded for the worktree checkout"
        );
        assert!(!s.editor.changes.loading, "changes loading settled");
        assert_eq!(
            s.editor.changes.checkout.as_deref(),
            s.editor.checkout_key.as_deref(),
            "changes data belongs to the committed checkout key"
        );
        // The sidebar highlight tracks the same committed identity.
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key(&pa.id, &wt_path).as_str())
        );
    }
}

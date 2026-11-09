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
        // The second project starts collapsed so its Count badge + right
        // chevron render; "goko" is expanded and owns the active workspace.
        let mut expanded = HashMap::new();
        expanded.insert("proj-notes".to_string(), false);
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
            expanded,
            selected_session: Some("sess-devin-spike".to_string()),
            active_workspace: Some(data::checkout_key("proj-goko", data::GOKO_ROOT)),
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
    use crate::editor::navigation::{Guard, PendingNav};
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
}

//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls and a terminal stub, all
//! driven by mock data. Real daemon/Tauri wiring lands in a later milestone.

mod chrome;
mod data;
mod sidebar;
mod term;
mod theme;

use std::collections::HashMap;

use rune::{col, row, AppSpec, Cx, El};

use crate::data::Data;
use crate::theme::Ink;

/// The workspace column's content mode: the terminal stub, the editor
/// placeholder, or the app-level settings page that replaces the whole column
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
    /// Workspace column mode (terminal / editor placeholder / settings).
    view: View,
}

impl State {
    fn new() -> State {
        let ink = Ink::new();
        let data = data::mock_data();
        let lines = term::real_term_lines(&ink);
        // The second project starts collapsed so its Count badge + right
        // chevron render; "goko" is expanded and owns the active workspace.
        let mut expanded = HashMap::new();
        expanded.insert("proj-notes".to_string(), false);
        let open_tabs = data.tabs.clone();
        let active_tab = data.active_tab;
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

/// Root: header full-width, then [sidebar 320 | 8 px separator | workspace
/// column]. Settings replaces the whole column like App.tsx's settingsMode;
/// Editor/Terminal keep the tab strip + controls and swap only the content.
fn view(s: &State, _cx: &mut Cx<State>) -> El {
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
            placeholder(
                "Editor",
                "The editor workspace lands in a later milestone.",
                ink,
            )
        } else {
            term::terminal_area(s, ink)
        };
        col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(chrome::tab_bar(s, ink))
            .child(chrome::controls_bar(s, ink))
            .child(content)
    };
    col()
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
        )
}

fn main() {
    rune::run(AppSpec {
        families: ("Inter", "ui-monospace"),
        min_size: Some((760.0, 480.0)),
        ..AppSpec::new("Goko", (1512.0, 814.0), State::new, view)
    });
}

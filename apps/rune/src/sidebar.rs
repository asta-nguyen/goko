//! The project sidebar — a port of `features/projects/ProjectTree.tsx` plus the
//! nested `SessionItems`, `WorktreeRows`, `StateDot`/`StateBadge`, `AgentIcon`,
//! `CheckoutStatus` and `Count` pieces it composes. Metrics follow the TSX where
//! fidelity.rs deviated (8 px state dots, TONE/DOT colour maps, the real
//! label-collapse and other-sessions nesting quirks). Row states (hover / press /
//! active / selected) deliberately deviate from the TSX, which painted them
//! panel-on-panel and invisible; the TSX was fixed the same way. The sidebar uses
//! a compact 4-pt-grid scale (see the constants below), deliberately denser than
//! the TSX.

use std::collections::HashSet;
use std::path::PathBuf;

use app_core::projects::ProjectView;
use rune::{Color, El, ScrollSpec, TextStyle, Tx, col, el, row};
use rune_kit::MenuItem;

use crate::State;
use crate::data::{self, SessionView, WorktreeInfo};
use crate::editor::backend::{Job, Req};
use crate::editor::navigation::{self, PendingNav};
use crate::theme::{self, Ink, T11, T11M, T11ST, T12, T12M, T128};

/// The sidebar's default width in pixels (`rune.sidebar_width` restores a
/// different one on the next launch).
pub const DEFAULT_SIDEBAR_W: f32 = 320.0;
/// The narrowest the sidebar may be dragged to.
pub const MIN_SIDEBAR_W: f32 = 220.0;
/// The widest the sidebar may be dragged to.
pub const MAX_SIDEBAR_W: f32 = 480.0;
/// The settings key the width is stored under.
pub const SIDEBAR_WIDTH_KEY: &str = "rune.sidebar_width";
/// The settings key the selected workspace id is stored under.
pub const SELECTED_WORKSPACE_KEY: &str = "rune.selected_workspace";
pub const PROJECTS_SCROLL_KEY: &str = "sidebar-projects-scroll";

/// Clamp a sidebar width to the draggable range.
pub fn clamp_width(width: f32) -> f32 {
    width.clamp(MIN_SIDEBAR_W, MAX_SIDEBAR_W)
}

// ---------------------------------------------------------------------------
// Row actions — the hover-revealed menus
// ---------------------------------------------------------------------------

/// The row a menu belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowTarget {
    /// A registered project's root checkout.
    Project { project_id: String },
    /// A linked worktree of a project.
    Worktree { project_id: String, path: String },
    /// A daemon session.
    Session { session_id: String },
    /// The sidebar's workspace selector (no row identity of its own).
    Workspace,
}

/// What a row's action button opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// The row's other commands.
    More,
}

impl RowTarget {
    /// The row's identity — every action button's key carries it, so two
    /// rows never share one.
    pub fn identity(&self) -> String {
        match self {
            RowTarget::Project { project_id } => project_id.clone(),
            RowTarget::Worktree { path, .. } => path.clone(),
            RowTarget::Session { session_id } => session_id.clone(),
            RowTarget::Workspace => "workspace".to_string(),
        }
    }
}

/// A text the sidebar asks the user for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarInput {
    /// The command a custom session runs.
    CustomCommand { target: RowTarget },
    /// The name of a new worktree.
    NewWorktree { project_id: String },
    /// A named workspace's new name.
    WorkspaceRename { id: String },
}

/// A removal awaiting confirmation. The row's facts are captured when the
/// menu item is chosen; Confirm re-checks that the target still exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarConfirm {
    /// A linked worktree: its folder name, dirty state and attached sessions.
    RemoveWorktree {
        project_id: String,
        path: String,
        name: String,
        dirty: Option<bool>,
        sessions: usize,
    },
    /// A registered project: only its registration goes.
    RemoveProject { project_id: String, name: String },
}

/// The width of the action cell: the buttons are always there, so the row's
/// text never shifts when they appear.
const ACTIONS_W: f32 = 44.0;

/// A row's hover action: a small icon button that fades in with the row's
/// hover and never moves anything.
fn action_button(
    ink: &Ink,
    icon: &'static rune::Icon,
    label: &str,
    key: String,
    action: impl Fn(&mut State) + 'static,
) -> El {
    el().key(key)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover_group()
        .hover(ink.text.alpha(0.10))
        .press(ink.text.alpha(0.16))
        .press_scale(0.92)
        .group_opacity(0.0, 1.0)
        .role("button")
        .label(label)
        .help(label)
        .focusable(true)
        .tab_index(-1)
        .child(theme::ic(icon, 12.0, ink.muted).group_color(ink.text))
        .on_press(action)
}

/// The suffix that makes a repeated name distinct in its accessible label:
/// none for the first of a name, " (2)" for the second, " (3)" and so on.
/// Visible text keeps the bare name.
fn ordinal_suffix(earlier: usize) -> String {
    if earlier == 0 {
        String::new()
    } else {
        format!(" ({})", earlier + 1)
    }
}

/// The actions a project or worktree row offers: a session to start and
/// the row's other commands.
fn checkout_actions(ink: &Ink, target: RowTarget, name: &str) -> El {
    let new_session = target.clone();
    let more = target;
    let identity = new_session.identity();
    row()
        .w(ACTIONS_W)
        .h(ROW_H)
        .gap(2.0)
        .items_center()
        .child(action_button(
            ink,
            &theme::PLUS,
            &format!("New session in {name}"),
            format!("sidebar-action:new-session:{identity}"),
            move |s: &mut State| {
                let anchor = format!("sidebar-row:{}", new_session.identity());
                let width = row_picker_width(s);
                open_new_session(s, anchor, Some(new_session.clone()), width);
            },
        ))
        .child(action_button(
            ink,
            &theme::MORE,
            &format!("More actions for {name}"),
            format!("sidebar-action:more:{identity}"),
            move |s: &mut State| {
                s.open_menu(more.clone(), MenuKind::More);
            },
        ))
}

/// A session row's hover action (stop, close): an icon button with
/// icon hover coloring instead of a background wash.
fn session_action_button(
    ink: &Ink,
    icon: &'static rune::Icon,
    hover_color: Color,
    label: &str,
    key: String,
    action: impl Fn(&mut State) + 'static,
) -> El {
    el().key(key)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover_group()
        .press_scale(0.92)
        .group_opacity(0.0, 1.0)
        .role("button")
        .label(label)
        .help(label)
        .focusable(true)
        .tab_index(-1)
        .child(theme::ic(icon, 12.0, ink.muted).group_color(hover_color))
        .on_press(action)
}

/// The actions a session row offers: stop it and close its pane.
fn session_actions(ink: &Ink, session_id: String, title: &str) -> El {
    let stop = session_id.clone();
    let close = session_id.clone();
    row()
        .w(ACTIONS_W)
        .h(ROW_H)
        .gap(2.0)
        .items_center()
        .child(session_action_button(
            ink,
            &theme::SQUARE,
            ink.danger,
            &format!("Stop session {title}"),
            format!("sidebar-action:stop:{session_id}"),
            move |s: &mut State| {
                s.stop_session(&stop, false);
            },
        ))
        .child(session_action_button(
            ink,
            &theme::X,
            ink.danger,
            &format!("Close pane {title}"),
            format!("sidebar-action:close:{session_id}"),
            move |s: &mut State| {
                s.close_session(&close);
            },
        ))
}

/// The open menu, as a kit layer. `None` when no menu is open.
pub fn menu_layer(s: &State, layer: &mut Vec<El>) -> Option<El> {
    let (target, kind) = s.sidebar_menu.clone()?;
    // A command menu drops from the button that opened it, right-aligned.
    let mut menu = rune_kit::Menu::new("sidebar-menu")
        .anchor(format!("sidebar-action:more:{}", target.identity()))
        .placement(rune_kit::Placement::BottomEnd)
        .open(true);
    match kind {
        MenuKind::More => {
            let open_target = target.clone();
            menu = menu.item(MenuItem::new("Open Editor").on_select_tx(
                move |st: &mut State, tx: &mut Tx| {
                    st.sidebar_menu = None;
                    let (project_id, checkout) = match &open_target {
                        RowTarget::Project { project_id } => (
                            project_id.clone(),
                            st.editor
                                .projects
                                .iter()
                                .find(|project| project.project.id == *project_id)
                                .map(|project| PathBuf::from(&project.project.root)),
                        ),
                        RowTarget::Worktree { project_id, path } => {
                            (project_id.clone(), Some(PathBuf::from(path)))
                        }
                        RowTarget::Session { .. } | RowTarget::Workspace => (String::new(), None),
                    };
                    let Some(checkout) = checkout else {
                        return;
                    };
                    navigation::navigate(
                        st,
                        PendingNav::SelectCheckout {
                            project_id,
                            checkout,
                        },
                        tx,
                    );
                },
            ));
            let project_id = match &target {
                RowTarget::Project { project_id } | RowTarget::Worktree { project_id, .. } => {
                    project_id.clone()
                }
                RowTarget::Session { .. } | RowTarget::Workspace => String::new(),
            };
            if !project_id.is_empty() {
                let pid = project_id.clone();
                menu = menu.item(MenuItem::new("Refresh worktrees").on_select_tx(
                    move |st: &mut State, _tx: &mut Tx| {
                        st.sidebar_menu = None;
                        st.editor.request_worktrees(&pid);
                    },
                ));
                // Offered only while Git still records a folder that is gone.
                let any_missing = s
                    .editor
                    .sidebar_worktrees
                    .get(project_id.as_str())
                    .and_then(|e| e.list.as_ref())
                    .is_some_and(|l| l.worktrees.iter().any(|w| w.missing));
                if any_missing {
                    let pid = project_id.clone();
                    menu = menu.item(MenuItem::new("Prune missing worktrees").on_select_tx(
                        move |st: &mut State, _tx: &mut Tx| {
                            st.sidebar_menu = None;
                            st.prune_worktrees(&pid);
                        },
                    ));
                }
                let pid = project_id.clone();
                // The row's checkout seeds the form's base ref: a worktree
                // row uses its path, a project row its registered root.
                let row_checkout = match &target {
                    RowTarget::Worktree { path, .. } => path.clone(),
                    _ => s
                        .editor
                        .projects
                        .iter()
                        .find(|p| p.project.id == project_id)
                        .map(|p| p.project.root.clone())
                        .unwrap_or_default(),
                };
                menu = menu.item(MenuItem::new("Create worktree").on_select_tx(
                    move |st: &mut State, _tx: &mut Tx| {
                        st.sidebar_menu = None;
                        crate::editor::worktree_form::open(st, &pid, &row_checkout);
                    },
                ));
            }
            match &target {
                RowTarget::Worktree { project_id, path } => {
                    // The row's facts are captured now, so the confirmation
                    // shows what the user saw on the row.
                    let worktrees: &[WorktreeInfo] = s
                        .editor
                        .sidebar_worktrees
                        .get(project_id.as_str())
                        .and_then(|e| e.list.as_ref())
                        .map_or(&[], |l| l.worktrees.as_slice());
                    let wt = worktrees.iter().find(|w| w.path == *path);
                    // Sessions that have ended no longer run in the checkout.
                    let all: Vec<&SessionView> =
                        s.data.sessions.iter().filter(|sv| !sv.ended()).collect();
                    let confirm = SidebarConfirm::RemoveWorktree {
                        project_id: project_id.clone(),
                        path: path.clone(),
                        name: wt.map_or_else(|| path.clone(), |w| w.name.clone()),
                        dirty: wt.and_then(|w| w.dirty),
                        sessions: data::sessions_for_checkout(worktrees, &all, path).len(),
                    };
                    // The list refreshes too: the confirmation reads the live
                    // dirty state while it is open.
                    let refresh = project_id.clone();
                    menu = menu.item(MenuItem::new("Remove worktree").danger(true).on_select_tx(
                        move |st: &mut State, _tx: &mut Tx| {
                            st.sidebar_menu = None;
                            st.sidebar_confirm = Some(confirm.clone());
                            st.workspace_remove_confirm = None;
                            st.editor.request_worktrees(&refresh);
                        },
                    ));
                }
                RowTarget::Project { project_id } => {
                    // Move to workspace: only the other workspaces, so the
                    // row's current group is never a destination.
                    let current = s
                        .editor
                        .projects
                        .iter()
                        .find(|p| p.project.id == *project_id)
                        .map(|p| p.project.workspace_id.clone());
                    let destinations: Vec<rune_kit::MenuEntry> = s
                        .workspaces
                        .iter()
                        .filter(|workspace| Some(&workspace.id) != current.as_ref())
                        .map(|workspace| {
                            let pid = project_id.clone();
                            let wid = workspace.id.clone();
                            let label = s.workspace_label(workspace);
                            rune_kit::MenuEntry::item(MenuItem::new(label).on_select_tx(
                                move |st: &mut State, _tx: &mut Tx| {
                                    st.sidebar_menu = None;
                                    st.move_project(&pid, &wid);
                                },
                            ))
                        })
                        .collect();
                    if !destinations.is_empty() {
                        menu = menu
                            .item(MenuItem::new("Move to workspace").submenu(destinations, false));
                    }
                    let confirm = SidebarConfirm::RemoveProject {
                        project_id: project_id.clone(),
                        name: s
                            .editor
                            .projects
                            .iter()
                            .find(|p| p.project.id == *project_id)
                            .map_or_else(|| project_id.clone(), |p| p.project.name.clone()),
                    };
                    menu = menu.item(MenuItem::new("Remove project").danger(true).on_select_tx(
                        move |st: &mut State, _tx: &mut Tx| {
                            st.sidebar_menu = None;
                            st.sidebar_confirm = Some(confirm.clone());
                            st.workspace_remove_confirm = None;
                        },
                    ));
                }
                RowTarget::Session { .. } | RowTarget::Workspace => {}
            }
        }
    }
    Some(
        menu.on_close(|st: &mut State| st.sidebar_menu = None)
            .build(&s.editor.kit, layer),
    )
}

/// The sidebar's text prompt, as a kit layer: the command a custom session
/// runs. `NewWorktree` no longer mounts a prompt — it is the worktree
/// form's keyboard shield (see `worktree_form::open`).
pub fn input_layer(s: &State, ink: &Ink, layer: &mut Vec<El>) -> Option<El> {
    let input = s.sidebar_input.clone()?;
    let (placeholder, title) = match &input {
        SidebarInput::CustomCommand { .. } => ("Command to run…", "New session"),
        // `NewWorktree` is now the worktree form's keyboard shield (set by
        // `worktree_form::open` so `on_key` returns early) — no prompt
        // mounts for it; the name-only route is retired.
        SidebarInput::NewWorktree { .. } => return None,
        SidebarInput::WorkspaceRename { .. } => ("Workspace name…", "Rename workspace"),
    };
    let field = rune_kit::TextInput::new(&s.sidebar_text)
        .placeholder(placeholder)
        .width(None)
        .on_edit(|st: &mut State| -> &mut rune::edit::TextEdit { &mut st.sidebar_text })
        .on_key(input_key)
        .build(&s.editor.kit);
    // The scrim closes the prompt without taking the text with it.
    layer.push(rune_kit::scrim("sidebar-input", |st: &mut State| {
        st.sidebar_input = None;
    }));
    Some(
        col()
            .key("sidebar-input")
            .w(320.0)
            .gap(8.0)
            .padding(12.0)
            .radius(8.0)
            .fill(ink.panel)
            .child(theme::txt(title, T12M, ink.text))
            .child(field),
    )
}

/// The prompt's keys: Enter applies, Escape cancels.
fn input_key(s: &mut State, ev: &rune::KeyEvent, _tx: &mut Tx) -> bool {
    match ev.key {
        rune::Key::Enter => {
            let text = s.sidebar_text.text.trim().to_string();
            match s.sidebar_input.clone() {
                // A workspace name stays in its prompt until the worker's
                // reply: a failed write keeps the text for correction.
                Some(SidebarInput::WorkspaceRename { id }) => {
                    s.submit_workspace_rename(&id, &text);
                    true
                }
                _ => {
                    let input = s.sidebar_input.take();
                    s.sidebar_text = rune::edit::TextEdit::new("");
                    match input {
                        Some(SidebarInput::CustomCommand { target }) => {
                            s.launch_custom_session(target, &text);
                        }
                        Some(SidebarInput::NewWorktree { project_id }) => {
                            s.create_worktree(&project_id, &text);
                        }
                        _ => {}
                    }
                    true
                }
            }
        }
        rune::Key::Escape => {
            s.sidebar_input = None;
            true
        }
        _ => false,
    }
}

// Density: the compact scale. Heights and vertical offsets are multiples of 4 pt so rows,
// badges and text land on whole device pixels at 1x/1.25x/1.5x/2x (boxes snap to device
// pixels but text keeps its exact position, so fractional layouts drift text off-centre).
const ROW_H: f32 = 28.0; // every row's pitch: project, worktree, session, "Worktrees"
// Filled rows (active, selected) keep a 4 pt gap so neighbouring fills never merge into
// one blob. The fill is an inset `row_layer` (`ROW_GAP` in from the top and bottom of
// whatever height its row is) rather than a margin on the row, so every row keeps its
// exact pitch and the text/badges inside never move off the pixel grid.
const ROW_GAP: f32 = 2.0;
const CELL: f32 = 24.0; // chevron cell
const GUIDE: f32 = 15.0; // indent guide offset; ≈ under the chevron centre
const SESSION_PL: f32 = 20.0; // session icon indent inside its list (was 40)
const NAME: TextStyle = TextStyle::new(13.0, 500.0).lh(16.0); // was T14M
const SUB: TextStyle = T12.lh(16.0); // session title (was T12)
const BRANCH: TextStyle = T11.lh(16.0); // inline branch (was T11, on its own line)
// Header / meta text on the same 16 pt line box, so it sits on whole pixels in its row.
const SUBS: TextStyle = T11.lh(16.0); // the session's sub-agents
const HDR: TextStyle = T11ST.lh(16.0); // "PROJECTS", "Unassigned"
const HDR_M: TextStyle = T11M.lh(16.0); // "Other sessions"

/// Hover lifts the TEXT, never the background. A background hover can't be told apart
/// from (or blur into) a filled neighbour (active, selected); a text lift reads on any
/// fill. Names go to near-white, muted text and glyphs to the text colour, accent text
/// brightens. Rows opt in with `hover_group()` and tag their text/glyphs with
/// `group_color(..)`; the hover lands at once and fades out over 0.1 s.
fn lit(ink: &Ink) -> Color {
    ink.text.mix(ink.white, 0.75)
}
fn lit_accent(ink: &Ink) -> Color {
    ink.accent.mix(ink.white, 0.4)
}

/// Press is the one background wash left: a brief flash is feedback, not a state.
fn press_wash(ink: &Ink) -> Color {
    ink.text.alpha(0.10)
}

/// "Add Project": the same text-hover as the rows (icon and label brighten).
/// Sends the real `Req::AddProject` through the editor backend — the folder
/// dialog result is a path, registration stays worker-side like the
/// editor's own picker.
pub(crate) fn add_project(ink: &Ink) -> El {
    row()
        .key("sidebar-add-project")
        .h(ROW_H)
        .pl(8.0)
        .pr(10.0)
        .gap(4.0)
        .items_center()
        .radius(6.0)
        .hover_group()
        .press(ink.text.alpha(0.12))
        .child(theme::ic(&theme::FOLDER_PLUS, 14.0, ink.text).group_color(lit(ink)))
        .child(theme::txt("Add Project", T128, ink.text).group_color(lit(ink)))
        .label("Add Project")
        .on_press_tx(|_s: &mut State, tx: &mut Tx| {
            tx.open_folder(
                rune::file_dialog::FileDialogOptions {
                    title: "Choose a project folder".into(),
                    ..Default::default()
                },
                |st: &mut State, result: rune::file_dialog::FileDialogResult, _tx: &mut Tx| {
                    match result {
                        Ok(Some(path)) => {
                            st.editor.backend.send(Job::project(Req::AddProject {
                                path: path.display().to_string(),
                                workspace_id: st.workspace_id().to_string(),
                            }));
                        }
                        Ok(None) => {}
                        Err(e) => st.notice = Some(e.to_string()),
                    }
                },
            );
        })
}

/// Disclosure chevron. The glyph, not a box, takes the hover (muted → text), so a
/// hovered row never carries a second hover surface inside it; the 20 px cell stays
/// the hit target and shrinks a touch while pressed. Toggling stays instant: this is
/// used many times a day, so motion would only slow it down.
fn chevron(open: bool, name: &str, ink: &Ink) -> El {
    el().size(CELL, CELL)
        .center()
        .hover_group()
        .press_scale(0.9)
        .role("button")
        .label(if open {
            format!("Collapse {name}")
        } else {
            format!("Expand {name}")
        })
        .child(
            theme::ic(
                if open {
                    &theme::CHEVRON_DOWN
                } else {
                    &theme::CHEVRON_RIGHT
                },
                12.0,
                ink.muted,
            )
            .group_color(ink.text),
        )
}

/// The workspace being viewed gets a 2 px accent edge, inset 3 px top and bottom of
/// the 22 pt row (so both ends sit 4 pt in from the 24 pt pitch: whole device pixels).
///
/// It is always mounted and only its alpha changes, so it moves with the row's fill
/// on the same colour spring instead of fading in on its own mount transition.
fn active_edge(ink: &Ink, active: bool) -> El {
    col().absolute(0.0, 0.0).w(2.0).h_full().py(3.0).child(
        el().w_full()
            .grow(1.0)
            .radius(1.0)
            .fill(ink.accent.alpha(if active { 1.0 } else { 0.0 })),
    )
}

/// A row's resting fill: the active wash, or the same colour at zero alpha so the
/// change is a colour transition rather than a fill appearing from nothing.
fn row_fill(ink: &Ink, active: bool) -> Color {
    ink.text.alpha(if active { 0.07 } else { 0.0 })
}

/// The row's background: an inset, rounded layer under its content (add it as the
/// row's first child). It carries the active / selected fill on the quick state
/// spring and leaves a `ROW_GAP` channel above and below, so two filled rows stacked
/// directly on each other still read as two. `row_h` is the ROW's height: a two-line
/// row (a project name over its branch) keeps the same channels around a taller fill,
/// and the `active_edge` inside tracks it through `h_full`.
fn row_layer(fill: Color, row_h: f32) -> El {
    el().absolute(0.0, ROW_GAP)
        .w_full()
        .h(row_h - 2.0 * ROW_GAP)
        .radius(6.0)
        .fill(fill)
        .motion(theme::STATE_SPRING)
}

/// `aside` (bg-panel) > section `flex min-h-0 flex-1 flex-col gap-2 p-3`.
///
/// Projects come from the editor's registered-project list — the same real
/// `ProjectView`/`WorktreeList` data the pickers use — never from the mock
/// `Data::projects`. Mock sessions can never join a real project id, so
/// they all render under Unassigned and no fake session is ever associated
/// with a registered project.
pub fn sidebar(s: &State, ink: &Ink, layer: &mut Vec<El>) -> El {
    let known: HashSet<&str> = s
        .editor
        .projects
        .iter()
        .map(|p| p.project.id.as_str())
        .collect();
    let (by_project, unassigned) = data::partition(&s.data, &known);
    let selected = s.workspace_id();

    let empty: &[&SessionView] = &[];
    let mut list = col()
        .key(PROJECTS_SCROLL_KEY)
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .gap(4.0)
        .pr(8.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if let Some(err) = &s.workspaces_error {
        list = list.child(status_row(err, ink.danger));
    }
    let visible: Vec<&ProjectView> = s
        .editor
        .projects
        .iter()
        .filter(|project| project.project.workspace_id == selected)
        .collect();
    if visible.is_empty() {
        // Loading / error / empty-group are distinct honest states — the
        // list never fabricates a project row.
        let (text, tone) = if s.editor.projects_loading {
            ("Loading projects…".to_string(), ink.muted)
        } else if let Some(err) = &s.editor.projects_error {
            (err.clone(), ink.danger)
        } else if s.editor.projects.is_empty() {
            ("No projects registered.".to_string(), ink.muted)
        } else {
            (format!("No projects in {}.", s.workspace_name()), ink.muted)
        };
        list = list.child(status_row(&text, tone));
    } else if let Some(err) = &s.editor.projects_error {
        list = list.child(status_row(err, ink.danger));
    }
    for project in &visible {
        let sessions = if s
            .active_project_in_view()
            .is_none_or(|id| id == project.project.id)
        {
            by_project
                .get(project.project.id.as_str())
                .map_or(empty, Vec::as_slice)
        } else {
            empty
        };
        // Projects sharing a name in this list number their labels apart.
        let earlier = visible
            .iter()
            .take_while(|p| p.project.id != project.project.id)
            .filter(|p| p.project.name == project.project.name)
            .count();
        let suffix = ordinal_suffix(earlier);
        list = list.child(project_node(s, ink, project, sessions, &suffix));
    }
    // Terminals started outside every project belong to the workspace they
    // were started in (older ones to Default) and show there unless one of
    // its projects is active (the per-project filter) — as a herdr space
    // shows its terminal before it has a repository.
    let loose: Vec<&SessionView> = unassigned
        .into_iter()
        .filter(|sv| s.loose_in_selected_workspace(sv))
        .collect();
    if s.active_project_in_view().is_none() && !loose.is_empty() {
        list = list.child(unassigned_node(s, ink, &loose));
    }

    let mut header = col().w_full().gap(4.0).child(
        row()
            .w_full()
            .items_center()
            .gap(4.0)
            .child(workspace_picker(s, ink, layer).grow(1.0).min_w(0.0))
            .child(
                el().key("project-map-button")
                    .size(28.0, 28.0)
                    .center()
                    .role("button")
                    .label("Project Map")
                    .help("Project Map (⌘K)")
                    .focusable(true)
                    .disabled(!crate::project_map::available(s))
                    .hover(ink.text.alpha(0.08))
                    .press(ink.text.alpha(0.12))
                    .on_press_tx(crate::project_map::open)
                    .child(theme::ic(&theme::FOLDER_TREE, 14.0, ink.muted)),
            ),
    );
    if let Some(id) = &s.workspace_remove_confirm {
        header = header.child(workspace_remove_confirm_row(s, ink, id));
    }
    if let Some(confirm) = &s.sidebar_confirm {
        header = header.child(sidebar_confirm_row(s, ink, confirm));
    }
    col().w(s.sidebar_width).h_full().fill(ink.panel).child(
        col()
            .w_full()
            .h_full()
            .min_h(0.0)
            .gap(4.0)
            .padding(8.0)
            .child(header)
            // header row: PROJECTS, its count and `+` (shell, agent or command).
            .child(
                row()
                    .key(PROJECTS_HEADER_KEY)
                    .w_full()
                    .h(ROW_H)
                    .px(4.0)
                    .items_center()
                    .justify_between()
                    .child(theme::txt("PROJECTS", HDR, ink.muted))
                    .child(
                        row()
                            .items_center()
                            .gap(4.0)
                            .children(
                                (!s.editor.projects.is_empty())
                                    .then(|| theme::count_badge(s.editor.projects.len(), ink)),
                            )
                            .child(
                                el().key("sidebar-projects-new-session")
                                    .size(20.0, 20.0)
                                    .center()
                                    .radius(4.0)
                                    .hover_group()
                                    .hover(ink.text.alpha(0.10))
                                    .press(ink.text.alpha(0.16))
                                    .press_scale(0.92)
                                    .role("button")
                                    .label("New session")
                                    .help("New session: shell, agent or command")
                                    .focusable(true)
                                    .expanded(
                                        s.new_session_picker.open
                                            && s.new_session_from
                                                .as_ref()
                                                .is_some_and(|f| f.anchor == PROJECTS_HEADER_KEY),
                                    )
                                    .child(
                                        theme::ic(&theme::PLUS, 12.0, ink.muted)
                                            .group_color(ink.text),
                                    )
                                    .on_press(|s: &mut State| {
                                        let width = picker_width(s);
                                        open_new_session(
                                            s,
                                            PROJECTS_HEADER_KEY.to_string(),
                                            None,
                                            width,
                                        );
                                    }),
                            ),
                    ),
            )
            .child(list),
    )
}

/// The sidebar's pickers are rune-kit's searchable select, spanning the
/// sidebar's content. Right-aligned under the PROJECTS header (8 pt in from
/// the sidebar's edge), the surface reaches 12 pt from the window's left —
/// rune keeps overlays 8 pt inside it, and a wider one would flip to open
/// rightwards, off the sidebar.
fn picker_width(s: &State) -> f32 {
    (s.sidebar_width - 20.0).max(200.0)
}

/// A row's picker opens under the row, whose right edge sits 8 pt further in
/// (the list's scroll gutter): 8 pt narrower keeps the same left margin.
fn row_picker_width(s: &State) -> f32 {
    (s.sidebar_width - 28.0).max(200.0)
}

/// The PROJECTS header row: the `+` picker drops from it, spanning the sidebar.
const PROJECTS_HEADER_KEY: &str = "sidebar-projects-header";

/// The new-session picker's option for a custom command; agent ids never
/// start with `:`.
const PICK_CUSTOM: &str = ":custom";
/// The workspace picker's actions; workspace ids never start with `:`.
const PICK_NEW_WORKSPACE: &str = ":new";
const PICK_RENAME_WORKSPACE: &str = ":rename";
const PICK_REMOVE_WORKSPACE: &str = ":remove";

/// Where the one new-session picker is open: the element it drops from, how
/// wide it is, and the checkout it starts in — `None` for "where the app is
/// looking" (the active project when it is in the selected workspace, else a
/// terminal of the selected workspace).
#[derive(Clone, Debug)]
pub struct NewSessionFrom {
    pub anchor: String,
    pub target: Option<RowTarget>,
    pub width: f32,
}

/// Open the new-session picker under `anchor`. Every "start something"
/// button — the PROJECTS `+`, a row's `+`, the tab bar's New tab caret —
/// opens this same searchable select.
pub fn open_new_session(s: &mut State, anchor: String, target: Option<RowTarget>, width: f32) {
    s.sidebar_menu = None;
    s.new_session_from = Some(NewSessionFrom {
        anchor,
        target,
        width,
    });
    s.new_session_picker.show(None);
}

/// The new-session picker, as a kit layer: Shell first, then every agent (one
/// whose binary is missing is listed, disabled, with the reason), then a
/// custom command. It drops right-aligned from the element that opened it.
pub fn new_session_layer(s: &State, layer: &mut Vec<El>) {
    let Some(from) = &s.new_session_from else {
        return;
    };
    if !s.new_session_picker.open {
        return;
    }
    let mut agents: Vec<&data::AgentInfo> = s.data.agents.iter().collect();
    agents.sort_by_key(|agent| agent.id != "shell");
    let mut options: Vec<rune_kit::ComboboxOption> = agents
        .into_iter()
        .map(|agent| {
            let mut option = rune_kit::ComboboxOption::new(&agent.id, &agent.label);
            if !agent.available {
                option.disabled = true;
                if let Some(binary) = &agent.binary {
                    option.detail = format!("{binary} not found on PATH");
                }
            }
            option
        })
        .collect();
    options.push(rune_kit::ComboboxOption::new(
        PICK_CUSTOM,
        "Custom command…",
    ));
    let _ = rune_kit::Combobox::new(
        "new-session",
        &s.new_session_picker,
        &options,
        |st: &mut State| &mut st.new_session_picker,
    )
    .label("New session")
    .search_placeholder("Shell, agent or command")
    .width(from.width)
    .placement(rune_kit::Placement::BottomEnd)
    .anchor(from.anchor.clone())
    .on_choose(|st: &mut State, id: &str, _tx: &mut Tx| start_picked_session(st, id))
    .build(&s.editor.kit, layer);
}

/// Start what the new-session picker chose. From a row: in that row's
/// checkout. Otherwise where the app is looking: in the active project when
/// it is in the selected workspace, else as a terminal of the selected
/// workspace in `$HOME` (a shell then takes a generated name). Either way its
/// tab and row show.
fn start_picked_session(s: &mut State, id: &str) {
    let from_row = s.new_session_from.take().and_then(|from| from.target);
    let in_view = from_row.is_none();
    let target = from_row.unwrap_or_else(|| {
        s.active_project_in_view()
            .and_then(|_| s.active_checkout_target())
            .unwrap_or(RowTarget::Workspace)
    });
    s.view = crate::View::Terminal;
    match id {
        PICK_CUSTOM => s.ask_for_input(SidebarInput::CustomCommand { target }),
        "shell" if in_view => s.new_shell_tab(),
        agent => s.request_session_argv(target, agent.to_string()),
    }
}

/// The workspace selector: a searchable select of every workspace (another
/// one's waiting sessions named beside it), then New, Rename and Remove.
fn workspace_picker(s: &State, ink: &Ink, layer: &mut Vec<El>) -> El {
    let selected = s.workspace_id().to_string();
    let counts = s.workspace_waiting_counts();
    let mut options: Vec<rune_kit::ComboboxOption> = s
        .workspaces
        .iter()
        .map(|workspace| {
            let name = s.workspace_label(workspace);
            let label = match counts.get(&workspace.id) {
                Some(n) if *n > 0 && workspace.id != selected => {
                    format!("{name} · {n} need you")
                }
                _ => name,
            };
            rune_kit::ComboboxOption::new(&workspace.id, label)
        })
        .collect();
    options.push(rune_kit::ComboboxOption::new(
        PICK_NEW_WORKSPACE,
        "New workspace",
    ));
    if selected != app_core::store::DEFAULT_WORKSPACE_ID {
        options.push(rune_kit::ComboboxOption::new(
            PICK_RENAME_WORKSPACE,
            "Rename workspace…",
        ));
        options.push(rune_kit::ComboboxOption::new(
            PICK_REMOVE_WORKSPACE,
            "Remove workspace…",
        ));
    }
    rune_kit::Combobox::new(
        "sidebar-workspace",
        &s.workspace_picker,
        &options,
        |st: &mut State| &mut st.workspace_picker,
    )
    .current(Some(&selected))
    .label("Workspace")
    .search_placeholder("Switch workspace")
    .width(picker_width(s))
    .placement(rune_kit::Placement::BottomStart)
    .trigger(workspace_header(s, ink))
    .on_choose(|st: &mut State, id: &str, _tx: &mut Tx| match id {
        PICK_NEW_WORKSPACE => st.submit_workspace_create(""),
        PICK_RENAME_WORKSPACE => {
            let id = st.workspace_id().to_string();
            let name = st.workspace_name();
            st.ask_for_workspace_rename(&id, &name);
        }
        PICK_REMOVE_WORKSPACE => {
            st.sidebar_confirm = None;
            st.workspace_remove_confirm = Some(st.workspace_id().to_string());
        }
        id => st.select_workspace(id),
    })
    .build(&s.editor.kit, layer)
}

/// The workspace selector's face: the selected workspace's label and a caret.
/// The picker around it opens and closes it.
fn workspace_header(s: &State, ink: &Ink) -> El {
    let name = s.workspace_name().to_string();
    // A waiting session in a hidden workspace is flagged here: the selector
    // keeps its dot and label suffix until that session stops waiting.
    let counts = s.workspace_waiting_counts();
    let hidden_waiting = counts
        .iter()
        .any(|(id, n)| id != s.workspace_id() && *n > 0);
    let label = if hidden_waiting {
        format!("Workspace: {name}, another workspace needs you")
    } else {
        format!("Workspace: {name}")
    };
    row()
        .key("sidebar-workspace-selector")
        .w_full()
        .h(ROW_H)
        .px(4.0)
        .gap(6.0)
        .items_center()
        .radius(6.0)
        .role("button")
        .label(label)
        .help("Switch workspace")
        .focusable(true)
        .hover_group()
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .child(theme::ic(&theme::FOLDER_TREE, 14.0, ink.muted).group_color(ink.text))
        .child(
            theme::txt(&name, T12M, ink.text)
                .grow(1.0)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(name.clone()),
        )
        .children(hidden_waiting.then(|| {
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(ink.warning)
                .help("Another workspace needs you")
        }))
        .child(theme::ic(&theme::CHEVRON_DOWN, 12.0, ink.muted).group_color(ink.text))
}

/// The removal confirmation: what happens to the projects, and the two
/// buttons. Nothing is removed until Remove is pressed.
fn workspace_remove_confirm_row(s: &State, ink: &Ink, id: &str) -> El {
    let name = s
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .map_or_else(|| "this workspace".to_string(), |w| s.workspace_label(w));
    let remove_id = id.to_string();
    col()
        .w_full()
        .gap(4.0)
        .padding(6.0)
        .radius(6.0)
        .border(1.0, ink.border)
        .child(theme::txt(
            &format!(
                "Remove {name}? Its terminals and agents are closed; its projects move to Default."
            ),
            T11M,
            ink.muted,
        ))
        .child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(
                    small_button("Remove", ink.danger, ink, move |s: &mut State| {
                        s.remove_workspace(&remove_id);
                    })
                    .key("sidebar-workspace-remove-confirm"),
                )
                .child(
                    small_button("Cancel", ink.text, ink, |s: &mut State| {
                        s.workspace_remove_confirm = None;
                    })
                    .key("sidebar-workspace-remove-cancel"),
                ),
        )
}

/// The worktree or project removal confirmation: what goes, and the two
/// buttons. Nothing is removed until Remove is pressed.
fn sidebar_confirm_row(s: &State, ink: &Ink, confirm: &SidebarConfirm) -> El {
    let mut body = col().w_full().gap(2.0);
    match confirm {
        SidebarConfirm::RemoveWorktree {
            project_id,
            path,
            name,
            dirty,
            sessions,
        } => {
            // The live entry wins over the state captured when the menu was
            // chosen, so a refreshed list replaces a stale dirty flag.
            let live = s
                .editor
                .sidebar_worktrees
                .get(project_id.as_str())
                .and_then(|e| e.list.as_ref())
                .and_then(|l| l.worktrees.iter().find(|w| w.path == *path));
            let dirty = live.map_or(*dirty, |w| w.dirty);
            let status = match dirty {
                Some(true) => "It has uncommitted changes.",
                Some(false) => "It has no uncommitted changes.",
                None => "Git status unknown.",
            };
            let attached = match sessions {
                0 => "No sessions run in it.".to_string(),
                1 => "1 session runs in it.".to_string(),
                n => format!("{n} sessions run in it."),
            };
            body = body
                .child(theme::txt(&format!("Remove {name}?"), T11M, ink.text))
                .child(theme::txt(&format!("{status} {attached}"), T11M, ink.muted));
        }
        SidebarConfirm::RemoveProject { name, .. } => {
            body = body.child(theme::txt(
                &format!(
                    "Remove {name}? Only its registration is removed; its sessions become Unassigned and its files stay."
                ),
                T11M,
                ink.muted,
            ));
        }
    }
    col()
        .w_full()
        .gap(4.0)
        .padding(6.0)
        .radius(6.0)
        .border(1.0, ink.border)
        .child(body)
        .child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(
                    small_button("Remove", ink.danger, ink, |s: &mut State| {
                        s.confirm_sidebar_removal();
                    })
                    .key("sidebar-confirm-remove"),
                )
                .child(
                    small_button("Cancel", ink.text, ink, |s: &mut State| {
                        s.sidebar_confirm = None;
                    })
                    .key("sidebar-confirm-cancel"),
                ),
        )
}

/// A compact inline text button for the header's confirmation.
fn small_button(label: &str, tone: Color, ink: &Ink, action: impl Fn(&mut State) + 'static) -> El {
    row()
        .h(22.0)
        .px(8.0)
        .items_center()
        .radius(4.0)
        .role("button")
        .label(label)
        .focusable(true)
        .hover_group()
        .hover(ink.text.alpha(0.10))
        .press(ink.text.alpha(0.16))
        .child(theme::txt(label, T11M, tone).group_color(ink.white))
        .on_press(action)
}

/// ResizableSplit separator: an 8 px zone with a 1 px border line centred,
/// full height. Dragging it resizes the sidebar between 220 and 480 px; the
/// width is saved when the drag ends, so it survives the next launch.
pub fn separator(_s: &State, ink: &Ink) -> El {
    el().w(8.0)
        .h_full()
        .role("separator")
        .label("Resize sidebar")
        .focusable(true)
        .child(el().absolute(3.5, 0.0).w(1.0).h_full().fill(ink.border))
        .on_drag(|s: &mut State, d: &mut rune::Drag, tx: &mut Tx| {
            match d.phase {
                rune::Phase::Down => {
                    // The grab offset: the width follows the pointer from
                    // where the drag started.
                    d.data = s.sidebar_width;
                }
                rune::Phase::Move => {
                    tx.instant();
                    s.sidebar_width = clamp_width(d.data + d.dx);
                }
                rune::Phase::Up => s.persist_sidebar_width(),
            }
        })
        // The arrow keys resize the sidebar too, like AppKit's split views.
        .on_step(|s: &mut State, steps, _| {
            s.sidebar_width = clamp_width(s.sidebar_width + steps as f32 * 8.0);
            s.persist_sidebar_width();
        })
}

// ---------------------------------------------------------------------------
// ProjectNode
// ---------------------------------------------------------------------------

/// One-line status inside the project list: initial loading, a projects
/// error, or the empty registry. Never a fabricated project row.
fn status_row(text: &str, tone: Color) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .items_center()
        .child(theme::txt(text, T12, tone).min_w(0.0).truncate_middle())
}

fn project_node(
    s: &State,
    ink: &Ink,
    project: &ProjectView,
    sessions: &[&SessionView],
    name_suffix: &str,
) -> El {
    let pid = project.project.id.as_str();
    // The accessible name: the project's name plus its repeat ordinal.
    let accessible = format!("{}{name_suffix}", project.project.name);
    let expanded = s.expanded.get(pid).copied().unwrap_or(true);
    // The sidebar's own per-project worktree state — loaded by the Projects
    // reply's ensure pass and `request_worktrees`, never by position, so a
    // late reply can only land on the project it was asked for.
    let entry = s.editor.sidebar_worktrees.get(pid);
    let wt_list = entry.and_then(|e| e.list.as_ref());
    let is_git = wt_list.is_some_and(|w| w.repo);
    let worktrees: &[WorktreeInfo] = wt_list.map_or(&[], |w| w.worktrees.as_slice());
    let root = worktrees.iter().find(|w| w.is_root);
    let root_path = root.map_or(project.project.root.as_str(), |r| r.path.as_str());

    // rootSessions: checkout-filtered for git projects with a root entry,
    // otherwise every session the project owns.
    let root_sessions: Vec<&SessionView> = if is_git && root.is_some() {
        data::sessions_for_checkout(worktrees, sessions, root_path)
    } else {
        sessions.to_vec()
    };
    // otherSessions: git projects only — sessions whose cwd matches no checkout.
    let other_sessions: Vec<&SessionView> = if is_git {
        sessions
            .iter()
            .copied()
            .filter(|sv| data::checkout_for_cwd(worktrees, &sv.info.cwd).is_none())
            .collect()
    } else {
        Vec::new()
    };

    let workspace_id = data::checkout_key(pid, &project.project.root);
    let active = s.active_workspace.as_deref() == Some(workspace_id.as_str());
    let branch_el = branch_subtitle(is_git, root, ink);

    // Row: `flex items-center gap-1 rounded-md px-1 py-0.5` (gap 2, px 4, h 24).
    // Git projects carry the branch under the name, so the row grows to fit
    // two lines; the rest keep the shared 28 pt pitch. `row_h` is computed once
    // so the fill and the active edge cover the branch line too — the fixed
    // pitch left the wash short and the trailing icons looked pushed off it.
    let row_h = if branch_el.is_some() { 40.0 } else { ROW_H };
    let mut row_el = row()
        .w_full()
        .h(row_h)
        .px(4.0)
        .gap(2.0)
        .items_center()
        .radius(6.0)
        .hover_group()
        // The whole row is one press target, so it lights up as one surface (`press`
        // only paints on the element that owns the press handler). The chevron keeps
        // its own handler and captures its own presses.
        .press(press_wash(ink))
        .child(
            row_layer(row_fill(ink, active), row_h)
                // A row's new-session picker drops from the whole row.
                .key(format!("sidebar-row:{pid}"))
                .child(active_edge(ink, active)),
        );

    // Chevron button 20×20; expanded chevron-down, collapsed chevron-right.
    let pid_toggle = pid.to_string();
    row_el = row_el.child(
        chevron(expanded, &accessible, ink).on_press(move |s: &mut State| {
            let cur = s.expanded.get(pid_toggle.as_str()).copied().unwrap_or(true);
            s.expanded.insert(pid_toggle.clone(), !cur);
        }),
    );

    // Label cell: name (+ checkout status) over the branch in small muted
    // text — the TSX's `active` → bg-secondary/70 was panel-on-panel and
    // invisible, and its press state only covered this cell; the fill, edge
    // and press now belong to the whole `row_el`.
    let mut texts = col().grow(1.0).min_w(0.0);
    texts = texts.child(
        row()
            .w_full()
            .min_w(0.0)
            .gap(6.0)
            .items_center()
            .child(
                theme::txt(&project.project.name, NAME, ink.text)
                    .group_color(lit(ink))
                    .min_w(40.0)
                    .truncate_middle()
                    .help_when_truncated(project.project.name.clone()),
            )
            .children(checkout_status(project.missing, root, ink)),
    );
    if let Some(branch) = branch_el {
        texts = texts.child(branch);
    }
    let activate = row()
        .grow(1.0)
        .min_w(0.0)
        .px(4.0)
        .gap(6.0)
        .items_center()
        .child(theme::ic(
            if is_git {
                &theme::FOLDER_GIT2
            } else {
                &theme::FOLDER
            },
            16.0,
            ink.accent,
        ))
        .child(texts);
    row_el = row_el.child(activate);

    // Collapsed && sessions>0 → Count badge.
    if !expanded && !sessions.is_empty() {
        // `mr(4)`: the row's own px(4) sits inside the label cell's px(4) for every
        // other right-hand element, so insets match (badge, context %, group badges).
        row_el = row_el.child(theme::count_badge(sessions.len(), ink).mr(4.0));
    }
    // The root row selects the project ROOT checkout: the press routes
    // through the draft guard as `SelectCheckout`, so a dirty draft prompts
    // before any identity mutation and Cancel restores the previous
    // workspace highlight along with everything else.
    let pid_select = pid.to_string();
    let root_checkout = PathBuf::from(&project.project.root);
    row_el = row_el
        .label(accessible.clone())
        .key(format!("sidebar-project:{pid}"))
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            navigation::navigate(
                s,
                PendingNav::SelectCheckout {
                    project_id: pid_select.clone(),
                    checkout: root_checkout.clone(),
                },
                tx,
            );
        });
    // The REVEAL hover-only action menus are opacity-0 by default — omitted,
    // matching the real app's resting render.
    row_el = row_el.child(checkout_actions(
        ink,
        RowTarget::Project {
            project_id: pid.to_string(),
        },
        &accessible,
    ));

    let mut node = col().child(row_el);
    // A failed worktree read for THIS project surfaces under its row —
    // same honest-error placement the desktop gives `worktreeQuery.error`.
    if let Some(err) = entry.and_then(|e| e.error.as_ref()) {
        node = node.child(
            row().ml(GUIDE + 6.0).px(8.0).h(ROW_H).items_center().child(
                theme::txt(err, T11M, ink.danger)
                    .min_w(0.0)
                    .truncate_middle(),
            ),
        );
    }
    if expanded {
        if !root_sessions.is_empty() {
            node = node.child(session_items(s, ink, &root_sessions, &workspace_id, true));
        }
        if let Some(group) = worktree_group(s, ink, pid, worktrees, sessions, name_suffix) {
            node = node.child(group);
        }
        if !other_sessions.is_empty() {
            node = node.child(other_sessions_group(s, ink, &other_sessions, &workspace_id));
        }
    }
    node
}

/// CheckoutStatus for a checkout row (and the project's root checkout):
/// `missing` → warning badge, dirty==true → 6 px warning dot, dirty==null →
/// 6 px muted dot, dirty==false → nothing.
fn checkout_status(missing: bool, root: Option<&WorktreeInfo>, ink: &Ink) -> Option<El> {
    if missing {
        return Some(theme::missing_badge(ink));
    }
    checkout_status_of(root?, ink)
}

fn checkout_status_of(checkout: &WorktreeInfo, ink: &Ink) -> Option<El> {
    if checkout.missing {
        return Some(theme::missing_badge(ink));
    }
    match checkout.dirty {
        Some(true) => Some(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(ink.warning)
                .help("Uncommitted changes"),
        ),
        None => Some(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(ink.muted)
                .help("Git status unknown"),
        ),
        Some(false) => None,
    }
}

/// Git project with a root worktree shows `root.branch ?? "detached <head8>"`.
fn branch_subtitle<'a>(is_git: bool, root: Option<&'a WorktreeInfo>, ink: &'a Ink) -> Option<El> {
    if !is_git {
        return None;
    }
    let root = root?;
    let label = root
        .branch
        .clone()
        .unwrap_or_else(|| format!("detached {}", &root.head[..8.min(root.head.len())]));
    Some(
        theme::txt(&label, BRANCH, ink.muted)
            .group_color(ink.text)
            .min_w(0.0)
            .truncate_middle()
            .help_when_truncated(label),
    )
}

// ---------------------------------------------------------------------------
// SessionItems / SessionRow / AgentIcon + StateDot
// ---------------------------------------------------------------------------

/// `ul.ml-4.border-l.border-border.pl-1.5.flex-col.gap-0.5` → ml 13, 1 px left
/// border, pl 6, gap 0.
fn session_items(
    s: &State,
    ink: &Ink,
    sessions: &[&SessionView],
    workspace_id: &str,
    indent: bool,
) -> El {
    let mut u = col()
        .ml(GUIDE)
        .pl(6.0)
        .gap(0.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], ink.border);
    for &sv in sessions {
        u = u.child(session_row(s, ink, sv, workspace_id, indent));
    }
    u
}

fn session_row(s: &State, ink: &Ink, sv: &SessionView, workspace_id: &str, indent: bool) -> El {
    let info = &sv.info;
    let selected = s.selected_session.as_deref() == Some(info.id.as_str());
    let ended = sv.ended();
    let title = sv.title();
    // Sessions titled alike number their accessible names apart, across the
    // whole sidebar: a title repeats in another project's list too.
    let earlier = s
        .data
        .sessions
        .iter()
        .take_while(|o| o.info.id != info.id)
        .filter(|o| o.title() == title)
        .count();
    let accessible = format!("{title}{}", ordinal_suffix(earlier));
    // showLabel: a label exists and differs from the title (case-insensitive).
    let label = sv
        .agent_label(&s.data.agents)
        .filter(|l| !l.eq_ignore_ascii_case(title.as_str()));
    // The account the session runs under, from its OWN launch argv — it
    // never follows a later active-account switch (services/accounts.ts
    // `accountNameOf`). Named-account argv carries the env folder; plain
    // argv means the Default account; a folder matching no stored account
    // shows its basename.
    let account = sv
        .meta
        .as_ref()
        .map(|meta| meta.agent.as_str())
        .filter(|agent| *agent == "claude" || *agent == "codex")
        .and_then(|agent| data::account_name_of(agent, &info.argv, s.data.accounts.as_ref()));
    // The row's label joins the agent name and the account (`Claude ·
    // work`), whichever parts exist.
    let label: Option<String> = match (label, account) {
        (Some(agent_label), Some(account)) => Some(format!("{agent_label} · {account}")),
        (Some(agent_label), None) => Some(agent_label.to_string()),
        (None, account) => account,
    };

    // `w-full flex items-center gap-2 rounded-md py-1.5` + `pl-10 pr-2`
    // (indent) or `px-2` (Unassigned). The TSX's `hover:bg-bg/60` painted a dark
    // box on the panel; here hover lifts the text (see `lit`) and only the press
    // flashes a wash — accent on the selected row, neutral otherwise.
    let press = if selected {
        ink.accent.alpha(0.10)
    } else {
        press_wash(ink)
    };
    // Label colour at rest / while the row is hovered.
    let (rest, hovered) = if selected {
        (ink.accent, lit_accent(ink))
    } else if ended {
        (ink.muted, ink.text)
    } else {
        (ink.text, lit(ink))
    };
    let mut b = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .h(ROW_H)
        .radius(6.0)
        .hover_group()
        .press(press)
        .label(accessible.clone());
    b = if indent {
        b.pl(SESSION_PL).pr(8.0)
    } else {
        b.px(8.0)
    };
    // bg-accent/10 + text-accent on the row; the fill is always present (alpha 0 when
    // not selected) so selecting is a colour transition, not a fill appearing.
    b = b.child(row_layer(
        ink.accent.alpha(if selected { 0.10 } else { 0.0 }),
        ROW_H,
    ));
    let sid = info.id.clone();
    let wid = workspace_id.to_string();
    b = b.on_press(move |s: &mut State| {
        s.selected_session = Some(sid.clone());
        s.active_workspace = Some(wid.clone());
        // Activating a session activates its workspace tab, opening one when
        // the session isn't open yet — like TerminalWorkspace's focus path.
        s.focus_session_tab(&sid);
    });

    let badge_bg = if selected {
        ink.panel.mix(ink.accent, 0.10)
    } else {
        ink.panel
    };
    b = b.child(agent_badge(
        sv,
        ink,
        badge_bg,
        s.spinner_angle(),
        s.session_shown_activity(&sv.info.id),
        &s.pulse,
    ));
    // The label cell takes every point the badge and the action cell leave:
    // its texts truncate inside it instead of pushing the actions off the
    // trailing edge.
    let mut texts = row().grow(1.0).min_w(0.0).gap(8.0).items_center();
    if let Some(l) = label {
        // label font-medium (sel→accent / ended→muted / else text) + title
        // `text-xs text-muted` truncate. The label truncates too: a long
        // `agent · account` name must elide inside the cell, not paint over
        // the actions.
        texts = texts
            .child(
                theme::txt(&l, NAME, rest)
                    .group_color(hovered)
                    .min_w(0.0)
                    .truncate_middle()
                    .help_when_truncated(l.clone()),
            )
            .child(
                theme::txt(&title, SUB, ink.muted)
                    .group_color(ink.text)
                    .min_w(0.0)
                    .truncate_middle()
                    .help_when_truncated(title.clone()),
            );
    } else {
        // single title font-medium, same colour rule.
        texts = texts.child(
            theme::txt(&title, NAME, rest)
                .group_color(hovered)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(title.clone()),
        );
    }
    // The agent CLIs running under the session: `↳ devin`, `↳ codex, devin ×2`.
    // Trailing meta inside the label cell — ahead of the name it read as a
    // wide gap between the icon and the label.
    let subagents = s.session_subagents(&sv.info.id);
    if !subagents.is_empty() {
        let label = app_core::subagents::subagent_label(&subagents);
        texts = texts.child(
            theme::txt(&format!("↳ {label}"), SUBS, ink.muted)
                .group_color(ink.text)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(label),
        );
    }
    b = b.child(texts);
    // The row's own actions: stop the session and close its pane. They sit
    // in a fixed cell at the trailing edge, so the row never shifts when
    // they appear.
    b = b.child(session_actions(ink, info.id.clone(), &accessible));
    col().child(b)
}

/// The 16×16 icon cell: AgentIcon plus the StateDot overlay — a 12 px disc in
/// `row_bg` (panel, or the selected row's accent tint, so no halo shows through)
/// under an 8 px dot, both centred at (14,14)
/// (`-bottom-0.5 -right-0.5 size-2.5 ring-2`).
fn agent_badge(
    s: &SessionView,
    ink: &Ink,
    row_bg: Color,
    angle: f32,
    activity: Option<app_core::activity::Activity>,
    pulse: &rune::Live<f32>,
) -> El {
    let (glyph, color) = theme::agent_glyph(s.agent_id(), ink);
    el().size(16.0, 16.0)
        .child(theme::ic(glyph, 16.0, color))
        // The state dot sits over the mark's corner, on a disc of the row's
        // own background so no glyph shows through it. The dot layer is
        // absolute so it centres ON the disc — as a flex sibling it would
        // sit beside it and read as stray dots on the mark.
        .child(
            el().absolute(6.0, 6.0)
                .size(10.0, 10.0)
                .center()
                .child(el().size(8.0, 8.0).radius(4.0).fill(row_bg))
                .child(
                    el().absolute(0.0, 0.0)
                        .size(10.0, 10.0)
                        .center()
                        .child(theme::state_dot(s.info.state, activity, angle, pulse, ink)),
                ),
        )
}

// ---------------------------------------------------------------------------
// WorktreeRows
// ---------------------------------------------------------------------------

/// `null` unless the project is a git repo with at least one linked worktree.
fn worktree_group(
    s: &State,
    ink: &Ink,
    project_id: &str,
    worktrees: &[WorktreeInfo],
    sessions: &[&SessionView],
    project_suffix: &str,
) -> Option<El> {
    let linked: Vec<&WorktreeInfo> = worktrees.iter().filter(|w| !w.is_root).collect();
    if linked.is_empty() {
        return None;
    }
    let group_open = s.worktree_groups.get(project_id).copied().unwrap_or(true);
    // Outer ul metrics; the whole header row is one toggle button.
    let mut outer = col()
        .ml(GUIDE)
        .pl(6.0)
        .gap(0.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], ink.border);
    let pid_group = project_id.to_string();
    outer = outer.child(
        row()
            .w_full()
            .items_center()
            .gap(2.0)
            .h(ROW_H)
            .px(4.0)
            .radius(6.0)
            .hover_group()
            .press(press_wash(ink))
            .label(format!(
                "{} worktrees of {}{}",
                if group_open { "Collapse" } else { "Expand" },
                s.editor
                    .projects
                    .iter()
                    .find(|p| p.project.id == project_id)
                    .map_or(project_id, |p| p.project.name.as_str()),
                project_suffix,
            ))
            .child(
                el().size(CELL, CELL).center().child(
                    theme::ic(
                        if group_open {
                            &theme::CHEVRON_DOWN
                        } else {
                            &theme::CHEVRON_RIGHT
                        },
                        12.0,
                        ink.muted,
                    )
                    .group_color(ink.text),
                ),
            )
            .child(
                row()
                    .grow(1.0)
                    .min_w(0.0)
                    .px(4.0)
                    .gap(6.0)
                    .items_center()
                    .child(theme::ic(&theme::FOLDER_TREE, 16.0, ink.muted).group_color(ink.text))
                    .child(
                        theme::txt("Worktrees", NAME, ink.text)
                            .group_color(lit(ink))
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate_middle(),
                    )
                    .child(theme::count_badge(linked.len(), ink)),
            )
            .on_press(move |s: &mut State| {
                let cur = s
                    .worktree_groups
                    .get(pid_group.as_str())
                    .copied()
                    .unwrap_or(true);
                s.worktree_groups.insert(pid_group.clone(), !cur);
            }),
    );
    if group_open {
        let mut inner = col()
            .ml(GUIDE)
            .pl(6.0)
            .gap(0.0)
            .border_sides([0.0, 0.0, 0.0, 1.0], ink.border);
        for (index, wt) in linked.iter().enumerate() {
            inner = inner.child(worktree_node(
                s, ink, project_id, wt, worktrees, sessions, index,
            ));
        }
        // The env-copy report a create left behind — the desktop's
        // `envResult` Alert under the project, including partial success.
        if let Some(notice) = s.editor.worktree_env.get(project_id) {
            inner = inner.child(crate::editor::worktree_form::env_notice_card(
                s, ink, project_id, notice,
            ));
        }
        outer = outer.child(inner);
    }
    Some(outer)
}

fn worktree_node(
    s: &State,
    ink: &Ink,
    project_id: &str,
    wt: &WorktreeInfo,
    worktrees: &[WorktreeInfo],
    sessions: &[&SessionView],
    index: usize,
) -> El {
    let checkout_sessions = data::sessions_for_checkout(worktrees, sessions, &wt.path);
    // Linked checkouts of this project that share the name number theirs apart.
    let earlier = worktrees
        .iter()
        .filter(|w| !w.is_root)
        .take_while(|w| w.path != wt.path)
        .filter(|w| w.name == wt.name)
        .count();
    let accessible = format!("{}{}", wt.name, ordinal_suffix(earlier));
    let workspace_id = data::checkout_key(project_id, &wt.path);
    let active = s.active_workspace.as_deref() == Some(workspace_id.as_str());
    let branch_label = wt
        .branch
        .clone()
        .unwrap_or_else(|| format!("detached {}", &wt.head[..8.min(wt.head.len())]));

    let mut text_row = row().grow(1.0).min_w(0.0).gap(6.0).items_center().child(
        theme::txt(&wt.name, NAME, ink.text)
            .group_color(lit(ink))
            .min_w(40.0)
            .truncate_middle()
            .help_when_truncated(wt.name.clone()),
    );
    if let Some(st) = checkout_status_of(wt, ink) {
        text_row = text_row.child(st);
    }
    // Subtitle only when the branch label differs from the worktree name.
    if branch_label != wt.name {
        text_row = text_row.child(
            theme::txt(&branch_label, BRANCH, ink.muted)
                .group_color(ink.text)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(branch_label.clone()),
        );
    }

    let wt_open = s
        .worktrees_open
        .get(wt.path.as_str())
        .copied()
        .unwrap_or(true);
    // attachedSessions spans every session (cross-project cwds included);
    // checkoutSessions is only this project's slice actually listed below.
    let all: Vec<&SessionView> = s.data.sessions.iter().collect();
    let attached = data::sessions_for_checkout(worktrees, &all, &wt.path);
    let show_count = !attached.is_empty() && (!wt_open || attached.len() > checkout_sessions.len());

    // Label cell only lays out; the fill, edge and press belong to the whole row.
    let activate = row().grow(1.0).min_w(0.0).px(4.0).gap(6.0).items_center();
    let wt_path = wt.path.clone();
    let mut row_el = row()
        .w_full()
        .items_center()
        .gap(2.0)
        .h(ROW_H)
        .px(4.0)
        .radius(6.0)
        .hover_group()
        .press(press_wash(ink))
        .label(accessible.clone())
        .child(
            row_layer(row_fill(ink, active), ROW_H)
                .key(format!("sidebar-row:{}", wt.path))
                .child(active_edge(ink, active)),
        )
        .child(
            chevron(wt_open, &accessible, ink).on_press(move |s: &mut State| {
                let cur = s
                    .worktrees_open
                    .get(wt_path.as_str())
                    .copied()
                    .unwrap_or(true);
                s.worktrees_open.insert(wt_path.clone(), !cur);
            }),
        )
        .child(
            activate
                .child(theme::ic(&theme::GIT_BRANCH, 16.0, ink.accent))
                .child(text_row),
        );
    if show_count {
        row_el = row_el.child(theme::count_badge(attached.len(), ink).mr(4.0));
    }
    row_el = row_el.child(checkout_actions(
        ink,
        RowTarget::Worktree {
            project_id: project_id.to_string(),
            path: wt.path.clone(),
        },
        &accessible,
    ));
    // A worktree row selects its EXACT registered checkout path — the press
    // routes through the same draft-guarded `SelectCheckout` as the project
    // root row, so identity and highlight only move together, post-guard.
    let pid_select = project_id.to_string();
    let wt_checkout = PathBuf::from(&wt.path);
    row_el = row_el
        .key(format!("sidebar-worktree:{project_id}:{index}"))
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            navigation::navigate(
                s,
                PendingNav::SelectCheckout {
                    project_id: pid_select.clone(),
                    checkout: wt_checkout.clone(),
                },
                tx,
            );
        });
    let mut node = col().child(row_el);
    if wt_open && !checkout_sessions.is_empty() {
        node = node.child(session_items(
            s,
            ink,
            &checkout_sessions,
            &workspace_id,
            true,
        ));
    }
    node
}

// ---------------------------------------------------------------------------
// "Other sessions" and "Unassigned" groups
// ---------------------------------------------------------------------------

/// `div.ml-4.border-l.pl-1.5` > header (`px-2 pb-1 pt-1`, 11 px medium muted)
/// + its own nested SessionItems — the real double-border quirk.
fn other_sessions_group(s: &State, ink: &Ink, sessions: &[&SessionView], workspace_id: &str) -> El {
    col()
        .ml(GUIDE)
        .pl(6.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], ink.border)
        .child(
            row()
                .h(ROW_H)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .child(theme::txt("Other sessions", HDR_M, ink.muted))
                .child(theme::count_badge(sessions.len(), ink)),
        )
        .child(session_items(s, ink, sessions, workspace_id, true))
}

/// `li` > header (`px-2 pb-1 pt-2`, 11 px semibold uppercase tracking-wider
/// muted) with a flex-1 "Unassigned" button + Count > SessionItems indent=false.
fn unassigned_node(s: &State, ink: &Ink, sessions: &[&SessionView]) -> El {
    let unassigned_active = s.active_workspace.as_deref() == Some(data::UNASSIGNED_WORKSPACE);
    let label = row()
        .grow(1.0)
        .h(ROW_H)
        .items_center()
        .px(4.0)
        .radius(6.0)
        .hover_group()
        .press(press_wash(ink))
        .label("Unassigned")
        .child(row_layer(row_fill(ink, unassigned_active), ROW_H))
        .child(
            theme::txt(
                "Unassigned",
                HDR,
                if unassigned_active {
                    ink.text
                } else {
                    ink.muted
                },
            )
            .group_color(if unassigned_active {
                lit(ink)
            } else {
                ink.text
            }),
        )
        .on_press(|s: &mut State| {
            s.active_workspace = Some(data::UNASSIGNED_WORKSPACE.to_string());
        })
        .key("sidebar-unassigned");
    col()
        .child(
            row()
                .pl(4.0)
                .pr(8.0)
                .pt(8.0)
                .h(ROW_H + 8.0)
                .gap(6.0)
                .items_center()
                .child(label)
                .child(theme::count_badge(sessions.len(), ink)),
        )
        .child(session_items(
            s,
            ink,
            sessions,
            data::UNASSIGNED_WORKSPACE,
            false,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_handle;

    #[test]
    fn the_sidebar_width_is_clamped_to_its_range() {
        assert_eq!(clamp_width(219.0), MIN_SIDEBAR_W);
        assert_eq!(clamp_width(220.0), 220.0);
        assert_eq!(clamp_width(320.0), 320.0);
        assert_eq!(clamp_width(480.0), 480.0);
        assert_eq!(clamp_width(481.0), MAX_SIDEBAR_W);
    }

    #[test]
    fn a_stored_width_is_applied_clamped() {
        let mut s = State::new(test_handle());
        s.apply_reply(crate::editor::backend::Reply {
            req: crate::editor::backend::Req::GetSetting {
                key: SIDEBAR_WIDTH_KEY.to_string(),
            },
            generation: 0,
            checkout_key: None,
            result: Ok(crate::editor::backend::ReplyData::Setting(Some(
                "300".to_string(),
            ))),
        });
        assert_eq!(s.sidebar_width, 300.0);

        // A width outside the range never reaches the layout.
        s.apply_reply(crate::editor::backend::Reply {
            req: crate::editor::backend::Req::GetSetting {
                key: SIDEBAR_WIDTH_KEY.to_string(),
            },
            generation: 0,
            checkout_key: None,
            result: Ok(crate::editor::backend::ReplyData::Setting(Some(
                "900".to_string(),
            ))),
        });
        assert_eq!(s.sidebar_width, MAX_SIDEBAR_W);
    }

    #[test]
    fn a_custom_command_needs_something_to_run() {
        let mut s = State::new(test_handle());
        let target = RowTarget::Project {
            project_id: "p1".into(),
        };
        s.launch_custom_session(target.clone(), "   ");
        assert!(s.notice.is_some(), "an empty command is refused");
        assert!(s.pending.is_empty(), "nothing was started");
    }

    /// A measured layout, not a source mirror: at the narrowest sidebar the
    /// session label must start right after the icon and the Stop/Close
    /// cell must sit at the row's trailing edge, not between them — the
    /// regression that left a wide blank before the name.
    #[test]
    fn a_session_row_lays_label_by_the_icon_and_actions_trailing() {
        use protocol::session::{SessionId, SessionInfo, SessionState};
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let mut s = State::new(test_handle());
        s.sidebar_width = MIN_SIDEBAR_W;
        // A registered agent + its Default account give the row the wide
        // `agent · account` label the narrow sidebar must truncate.
        s.data.agents.push(data::AgentInfo {
            id: "claude".into(),
            label: "Claude Code".into(),
            available: true,
            path: Some("claude".into()),
            binary: Some("claude".into()),
        });
        s.data.accounts = Some(app_core::accounts::AccountsView {
            claude: app_core::accounts::AgentView {
                active: "default".into(),
                items: vec![app_core::accounts::AccountView {
                    id: "default".into(),
                    name: "Default".into(),
                    dir: "/home/user/.claude".into(),
                    is_default: true,
                }],
            },
            codex: app_core::accounts::AgentView {
                active: String::new(),
                items: Vec::new(),
            },
        });
        let info = SessionInfo {
            id: SessionId::new(),
            title: "Work".into(),
            argv: vec!["claude".into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let key = data::SessionInfo::key(info.id);
        s.session_metas.insert(
            key.clone(),
            app_core::store::SessionMeta {
                session_id: key.clone(),
                project_id: None,
                agent: "claude".into(),
                argv: info.argv.clone(),
                cwd: info.cwd.clone(),
                title: info.title.clone(),
                created_at: 0,
                workspace_id: None,
            },
        );
        // A running sub-agent adds `↳ devin` inside the label cell.
        s.subagents.insert(
            key.clone(),
            vec![app_core::subagents::Subagent {
                agent: "devin".into(),
                pid: 4242,
                command: "devin".into(),
            }],
        );
        s.upsert_session(info);

        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let els = rt.elements();

        let text = |t: &str| {
            els.iter()
                .position(|e| e.text.as_deref() == Some(t))
                .unwrap_or_else(|| panic!("{t:?} not mounted"))
        };
        let label = &els[text("Claude Code · Default")];
        let title = &els[text("Work")];
        let subs = &els[text("↳ devin")];
        let stop_key = format!("sidebar-action:stop:{key}");
        let stop_idx = els
            .iter()
            .position(|e| e.key.as_deref() == Some(stop_key.as_str()))
            .expect("stop action mounted");
        // The action cell is the stop button's row; the label cell is the
        // label text's row; the session row is the label cell's parent.
        let actions = &els[els[stop_idx].parent.expect("actions cell")];
        let texts = &els[label.parent.expect("label cell")];
        let row_idx = texts.parent.expect("session row");
        let row = &els[row_idx];
        // The icon is the row's small square child ahead of the label cell.
        let badge = els
            .iter()
            .find(|e| e.parent == Some(row_idx) && e.rect[2] <= 20.0 && e.rect[3] <= 20.0)
            .expect("session icon");

        // The label starts one gap after the icon — never the wide channel
        // a mid-row action cell left before it.
        let after_icon = label.rect[0] - (badge.rect[0] + badge.rect[2]);
        assert!(
            (4.0..=9.0).contains(&after_icon),
            "label sits {after_icon:.1} pt after the icon, row {:?}",
            row.rect,
        );
        // Stop/Close sit in the row's trailing cell after the label cell —
        // they neither overlap the texts nor open a gap before them.
        let texts_right = texts.rect[0] + texts.rect[2];
        assert!(
            actions.rect[0] >= texts_right - 0.01,
            "actions {:?} overlap the label cell {:?}",
            actions.rect,
            texts.rect,
        );
        let row_right = row.rect[0] + row.rect[2];
        let actions_right = actions.rect[0] + actions.rect[2];
        assert!(
            (actions_right - (row_right - 8.0)).abs() < 0.5,
            "actions end at {actions_right:.1}, row's trailing edge {row_right:.1} - 8",
        );
        // A long `agent · account` label elides inside the label cell: its
        // allocated right bound stays ahead of the action cell.
        let label_right = label.rect[0] + label.rect[2];
        assert!(
            label_right <= texts_right + 0.01,
            "label's allocated box {label_right:.1} escapes the label cell {texts_right:.1}",
        );
        assert!(
            label_right <= actions.rect[0] + 0.01,
            "label's allocated box {label_right:.1} reaches the action cell {:?}",
            actions.rect,
        );
        // The title and the sub-agent line stay inside the label cell, after
        // the label — nothing reopens a gap before the name.
        assert!(subs.rect[0] >= title.rect[0], "↳ cell precedes the title");
        assert!(
            subs.rect[0] >= label.rect[0] + label.rect[2] - 0.01,
            "↳ cell precedes the label"
        );
    }

    /// A git project's branch subtitle grows its row to two lines; the wash
    /// and the active edge must cover the whole row, not the 28 pt pitch —
    /// the regression that left the branch hanging below the fill and the
    /// menu icons looking pushed off the wash's middle.
    #[test]
    fn a_branch_project_row_paints_its_fill_and_edge_full_height() {
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let mut s = State::new(test_handle());
        s.editor.projects = vec![ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: "web".into(),
                root: "/work/a/web".into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }];
        s.editor.sidebar_worktrees.insert(
            "p1".to_string(),
            crate::editor::state::SidebarWorktrees {
                list: Some(data::WorktreeList {
                    repo: true,
                    worktrees: vec![WorktreeInfo {
                        name: "web".into(),
                        path: "/work/a/web".into(),
                        branch: Some("feature/sidebar".into()),
                        head: "0123456789abcdef0123456789abcdef01234567".into(),
                        is_root: true,
                        missing: false,
                        dirty: Some(false),
                        changed: Some(0),
                    }],
                }),
                loading: false,
                error: None,
            },
        );
        // The active checkout paints the wash and the edge the bug starved.
        s.active_workspace = Some(data::checkout_key("p1", "/work/a/web"));

        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let els = rt.elements();

        let row_idx = els
            .iter()
            .position(|e| e.key.as_deref() == Some("sidebar-project:p1"))
            .expect("project row mounted");
        let row = &els[row_idx];
        assert!(
            (row.rect[3] - 40.0).abs() < 0.5,
            "branch project row {:?} is not the two-line 40 pt",
            row.rect,
        );

        // The wash is the row's widest child — the edge-to-edge absolute layer.
        let layer_idx = els
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent == Some(row_idx))
            .max_by(|a, b| a.1.rect[2].partial_cmp(&b.1.rect[2]).unwrap())
            .map(|(i, _)| i)
            .expect("fill layer mounted");
        let layer = &els[layer_idx];
        // It spans the row minus its 2 pt channels, not the 28 pt pitch that
        // left the branch's line bare.
        assert!(
            (layer.rect[3] - (row.rect[3] - 2.0 * ROW_GAP)).abs() < 0.5,
            "fill layer {:?} does not cover the row {:?}",
            layer.rect,
            row.rect,
        );
        assert!(
            (layer.rect[1] - (row.rect[1] + ROW_GAP)).abs() < 0.5,
            "fill layer top {:?} loses its gap under row {:?}",
            layer.rect,
            row.rect,
        );
        let layer_bottom = layer.rect[1] + layer.rect[3];
        let row_bottom = row.rect[1] + row.rect[3];
        assert!(
            (row_bottom - layer_bottom - ROW_GAP).abs() < 0.5,
            "fill layer bottom {layer_bottom} loses its gap under row bottom {row_bottom}",
        );
        let branch = els
            .iter()
            .find(|e| e.text.as_deref() == Some("feature/sidebar"))
            .expect("branch subtitle mounted");
        assert!(
            branch.rect[1] + branch.rect[3] <= layer_bottom + 0.01,
            "branch {:?} spills below the wash {:?}",
            branch.rect,
            layer.rect,
        );

        // The active edge is the 2 pt bar inside the layer: centred on the
        // row, spanning it minus its 3 pt insets — not the short pitch's bar.
        let edge_idx = els
            .iter()
            .position(|e| e.parent == Some(layer_idx) && e.rect[2] <= 3.0)
            .expect("active edge mounted");
        let bar = els
            .iter()
            .find(|e| e.parent == Some(edge_idx))
            .expect("edge bar mounted");
        assert!(
            (bar.rect[3] - (layer.rect[3] - 6.0)).abs() < 0.5,
            "edge bar {:?} does not track the layer {:?}",
            bar.rect,
            layer.rect,
        );
        let row_mid = row.rect[1] + row.rect[3] / 2.0;
        assert!(
            ((bar.rect[1] + bar.rect[3] / 2.0) - row_mid).abs() < 0.5,
            "edge bar {:?} is off the row's middle {row_mid}",
            bar.rect,
        );

        // The menu icons stay at the trailing edge, centred on the whole row —
        // exactly what the short wash made look misplaced.
        for key in ["sidebar-action:new-session:p1", "sidebar-action:more:p1"] {
            let btn = els
                .iter()
                .find(|e| e.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("{key} mounted"));
            assert!(
                ((btn.rect[1] + btn.rect[3] / 2.0) - row_mid).abs() < 0.5,
                "{key} {:?} is off the row's middle {row_mid}",
                btn.rect,
            );
        }
        let more = els
            .iter()
            .find(|e| e.key.as_deref() == Some("sidebar-action:more:p1"))
            .unwrap();
        let row_right = row.rect[0] + row.rect[2];
        let more_right = more.rect[0] + more.rect[2];
        assert!(
            more_right > row_right - 8.0 && more_right <= row_right - 3.0,
            "actions end at {more_right}, row's trailing edge {row_right} - px(4)",
        );
    }

    /// Two projects, one with a worktree group and a session: every sidebar
    /// button is named after its target, and no two action buttons share a
    /// name.
    #[test]
    fn every_sidebar_button_names_its_target() {
        use protocol::session::{SessionId, SessionInfo, SessionState};
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let project = |id: &str, name: &str, root: &str| ProjectView {
            project: app_core::store::Project {
                id: id.into(),
                name: name.into(),
                root: root.into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        };
        let checkout = |name: &str, path: &str, is_root: bool| WorktreeInfo {
            name: name.into(),
            path: path.into(),
            branch: Some(name.into()),
            head: "0123456789abcdef0123456789abcdef01234567".into(),
            is_root,
            missing: false,
            dirty: Some(false),
            changed: Some(0),
        };

        let mut s = State::new(test_handle());
        s.editor.projects = vec![
            project("p1", "goko", "/work/a/goko"),
            project("p2", "tools", "/work/a/tools"),
        ];
        s.editor.sidebar_worktrees.insert(
            "p1".to_string(),
            crate::editor::state::SidebarWorktrees {
                list: Some(data::WorktreeList {
                    repo: true,
                    worktrees: vec![
                        checkout("goko", "/work/a/goko", true),
                        checkout("goko-feat", "/work/a/goko-feat", false),
                    ],
                }),
                loading: false,
                error: None,
            },
        );
        s.upsert_session(SessionInfo {
            id: SessionId::new(),
            title: "Work".into(),
            argv: vec!["sh".into()],
            cwd: "/work/a/goko".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        });

        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let els = rt.elements();

        let buttons: Vec<_> = els.iter().filter(|e| e.role == "button").collect();
        for button in &buttons {
            assert!(
                button
                    .label
                    .as_deref()
                    .is_some_and(|l| !l.trim().is_empty()),
                "sidebar button {:?} has no name",
                button.key.as_deref().unwrap_or(button.path.as_str()),
            );
        }
        let mut action_names: Vec<&str> = buttons
            .iter()
            .filter(|b| {
                b.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("sidebar-action:"))
            })
            .filter_map(|b| b.label.as_deref())
            .collect();
        let total = action_names.len();
        action_names.sort_unstable();
        action_names.dedup();
        assert_eq!(
            action_names.len(),
            total,
            "two action buttons share a name: {action_names:?}"
        );
        for expected in [
            "Collapse goko",
            "Collapse tools",
            "Collapse worktrees of goko",
            "Collapse goko-feat",
            "New session in goko",
            "More actions for goko",
            "New session in goko-feat",
            "More actions for goko-feat",
            "New session in tools",
            "More actions for tools",
            "Stop session Work",
            "Close pane Work",
        ] {
            assert!(
                buttons.iter().any(|b| b.label.as_deref() == Some(expected)),
                "no sidebar button named {expected:?}",
            );
        }
    }

    /// Two projects both named `web`, each with two sessions titled `zsh`:
    /// no two buttons share an accessible name. The first of each name keeps
    /// the plain name; repeats count up from 2.
    #[test]
    fn repeated_names_get_numbered_accessible_labels() {
        use protocol::session::{SessionId, SessionInfo, SessionState};
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let project = |id: &str, root: &str| ProjectView {
            project: app_core::store::Project {
                id: id.into(),
                name: "web".into(),
                root: root.into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        };

        let mut s = State::new(test_handle());
        s.editor.projects = vec![project("p1", "/work/a/web"), project("p2", "/work/b/web")];
        for root in ["/work/a/web", "/work/b/web"] {
            for _ in 0..2 {
                s.upsert_session(SessionInfo {
                    id: SessionId::new(),
                    title: "zsh".into(),
                    argv: vec!["sh".into()],
                    cwd: root.into(),
                    state: SessionState::Running,
                    pid: None,
                    exit_code: None,
                    error: None,
                    started_at: 0,
                    ended_at: None,
                });
            }
        }

        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let names: Vec<String> = rt
            .elements()
            .iter()
            .filter(|e| e.role == "button")
            .filter_map(|e| e.label.clone())
            .collect();

        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            names.len(),
            "two buttons share a name: {names:?}"
        );
        for expected in [
            "Collapse web",
            "Collapse web (2)",
            "New session in web",
            "New session in web (2)",
            "More actions for web",
            "More actions for web (2)",
            "Stop session zsh",
            "Stop session zsh (2)",
            "Stop session zsh (3)",
            "Stop session zsh (4)",
            "Close pane zsh (4)",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "no sidebar button named {expected:?}: {names:?}",
            );
        }
    }

    /// The status marks explain themselves on hover, and a label cut short
    /// by its cell offers its full text.
    #[test]
    fn status_marks_and_truncated_labels_explain_themselves() {
        use protocol::session::{SessionId, SessionInfo, SessionState};
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let long_project = "a-project-name-far-too-long-for-the-narrow-sidebar";
        let long_worktree = "a-worktree-name-that-is-also-far-too-long";
        let long_title = "a session title that cannot fit the narrow row";
        let mut s = State::new(test_handle());
        s.sidebar_width = MIN_SIDEBAR_W;
        s.editor.projects = vec![ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: long_project.into(),
                root: "/work/a/long".into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }];
        let checkout = |name: &str, path: &str, is_root, missing, dirty| WorktreeInfo {
            name: name.into(),
            path: path.into(),
            branch: Some("main".into()),
            head: "0123456789abcdef0123456789abcdef01234567".into(),
            is_root,
            missing,
            dirty,
            changed: Some(0),
        };
        s.editor.sidebar_worktrees.insert(
            "p1".to_string(),
            crate::editor::state::SidebarWorktrees {
                list: Some(data::WorktreeList {
                    repo: true,
                    worktrees: vec![
                        // Dirty root: the project row's warning dot.
                        checkout(long_project, "/work/a/long", true, false, Some(true)),
                        // Unknown status: the muted dot.
                        checkout(long_worktree, "/work/a/wt", false, false, None),
                        // A linked worktree whose folder is gone: the badge.
                        checkout("gone", "/work/a/gone", false, true, Some(false)),
                    ],
                }),
                loading: false,
                error: None,
            },
        );
        s.upsert_session(SessionInfo {
            id: SessionId::new(),
            title: long_title.into(),
            argv: vec!["sh".into()],
            cwd: "/work/a/long".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        });

        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let els = rt.elements();
        let help_of = |text: &str| {
            els.iter()
                .find(|e| e.text.as_deref() == Some(text))
                .unwrap_or_else(|| panic!("{text:?} not mounted"))
                .help
                .clone()
        };
        let has_help = |text: &str| els.iter().any(|e| e.help.as_deref() == Some(text));

        assert!(has_help("Uncommitted changes"), "dirty checkout's dot");
        assert!(has_help("Git status unknown"), "unknown status' dot");
        assert!(
            has_help("This folder no longer exists on disk"),
            "missing badge",
        );
        // The element keeps the whole string (rune cuts it at paint time), so
        // the tooltip, copy and accessibility text are never the "…" version.
        assert_eq!(help_of(long_project).as_deref(), Some(long_project));
        assert_eq!(help_of(long_worktree).as_deref(), Some(long_worktree));
        assert_eq!(help_of(long_title).as_deref(), Some(long_title));
    }

    // -- workspace selection -------------------------------------------------

    use crate::editor::backend::{Reply, ReplyData, Req};

    fn workspace(id: &str, name: &str) -> app_core::store::Workspace {
        app_core::store::Workspace {
            id: id.into(),
            name: name.into(),
            created_at: 0,
        }
    }

    fn reply(req: Req, result: Result<ReplyData, String>) -> Reply {
        Reply {
            req,
            generation: 0,
            checkout_key: None,
            result,
        }
    }

    fn list_reply(list: Vec<app_core::store::Workspace>) -> Reply {
        reply(Req::Workspaces, Ok(ReplyData::Workspaces(list)))
    }

    fn saved_reply(id: &str) -> Reply {
        reply(
            Req::GetSetting {
                key: SELECTED_WORKSPACE_KEY.to_string(),
            },
            Ok(ReplyData::Setting(Some(id.to_string()))),
        )
    }

    #[test]
    fn a_workspaces_reply_selects_default_then_a_saved_id_restores() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", "Work"),
        ]));
        assert_eq!(s.workspace_id(), app_core::store::DEFAULT_WORKSPACE_ID);

        s.apply_reply(saved_reply("w1"));
        assert_eq!(s.workspace_id(), "w1", "the saved workspace restores");
    }

    #[test]
    fn a_missing_saved_workspace_falls_back_to_default() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![workspace(
            app_core::store::DEFAULT_WORKSPACE_ID,
            "Default",
        )]));
        s.apply_reply(saved_reply("gone"));
        assert_eq!(s.workspace_id(), app_core::store::DEFAULT_WORKSPACE_ID);
    }

    #[test]
    fn a_stale_restore_never_overrides_a_newer_selection() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", "Work"),
        ]));
        s.select_workspace("w1");
        // The startup read lands late: it must not pull the user back.
        s.apply_reply(saved_reply(app_core::store::DEFAULT_WORKSPACE_ID));
        assert_eq!(s.workspace_id(), "w1");
    }

    /// New workspace asks for no name: an unnamed one is created and selected,
    /// and reads "New workspace" until a project gives it a label.
    #[test]
    fn new_workspace_is_created_unnamed_selected_and_labelled_by_its_project() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![workspace(
            app_core::store::DEFAULT_WORKSPACE_ID,
            "Default",
        )]));
        s.submit_workspace_create("");
        let token = s.workspace_pending.expect("submission in flight");
        assert!(s.sidebar_input.is_none(), "no name prompt opens");

        s.apply_reply(reply(
            Req::CreateWorkspace {
                name: String::new(),
                token,
            },
            Ok(ReplyData::WorkspaceCreated(workspace("w1", ""))),
        ));
        assert_eq!(s.workspace_id(), "w1", "the new workspace is selected");
        assert_eq!(s.workspace_name(), "New workspace");

        // A second empty one is told apart.
        s.workspaces.push(workspace("w2", ""));
        let w2 = s.workspaces[2].clone();
        assert_eq!(s.workspace_label(&w2), "New workspace 2");

        // Its first project names it, as herdr's space follows its folder.
        s.editor.projects.push(app_core::projects::ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: "goko".into(),
                root: "/src/goko".into(),
                workspace_id: "w1".into(),
                created_at: 0,
            },
            missing: false,
        });
        assert_eq!(s.workspace_name(), "goko");
        assert_eq!(s.workspace_label(&w2), "New workspace");
    }

    #[test]
    fn a_failed_workspace_create_keeps_the_selection() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![workspace(
            app_core::store::DEFAULT_WORKSPACE_ID,
            "Default",
        )]));
        s.submit_workspace_create("");
        let token = s.workspace_pending.expect("submission in flight");

        s.apply_reply(reply(
            Req::CreateWorkspace {
                name: String::new(),
                token,
            },
            Err("cannot create the workspace".into()),
        ));

        assert_eq!(s.workspace_id(), app_core::store::DEFAULT_WORKSPACE_ID);
        assert!(s.workspace_pending.is_none(), "another try can go out");
        assert!(s.notice.is_some(), "the reason is shown");
    }

    #[test]
    fn removing_the_selected_workspace_selects_default() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", "Work"),
        ]));
        s.select_workspace("w1");

        s.apply_reply(reply(
            Req::RemoveWorkspace { id: "w1".into() },
            Ok(ReplyData::Workspaces(vec![workspace(
                app_core::store::DEFAULT_WORKSPACE_ID,
                "Default",
            )])),
        ));

        assert_eq!(s.workspace_id(), app_core::store::DEFAULT_WORKSPACE_ID);
        assert!(!s.workspaces.iter().any(|w| w.id == "w1"));
    }

    #[test]
    fn a_failed_rename_keeps_the_selection_and_the_prompt() {
        let mut s = State::new(test_handle());
        s.apply_reply(list_reply(vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", "Work"),
        ]));
        s.select_workspace("w1");
        s.ask_for_workspace_rename("w1", "Work");
        s.submit_workspace_rename("w1", "Default");
        let token = s.workspace_pending.expect("submission in flight");

        s.apply_reply(reply(
            Req::RenameWorkspace {
                id: "w1".into(),
                name: "Default".into(),
                token,
            },
            Err("a workspace named 'Default' already exists".into()),
        ));

        assert_eq!(s.workspace_id(), "w1");
        assert!(matches!(
            s.sidebar_input,
            Some(SidebarInput::WorkspaceRename { .. })
        ));
        assert_eq!(s.sidebar_text.text, "Work");
    }

    /// The empty sidebar has no New Terminal button: the PROJECTS `+` is the
    /// one way to start something, and there is no Add Project button.
    #[test]
    fn the_empty_sidebar_offers_only_the_plus_picker() {
        use rune::{Cx, Env, Runtime};

        fn sidebar_view(s: &State, _: &mut Cx<State>) -> El {
            sidebar(s, &s.ink, &mut Vec::new())
        }

        let s = State::new(test_handle());
        let mut rt = Runtime::new(s, sidebar_view, (260.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let els = rt.elements();
        let has = |key: &str| els.iter().any(|e| e.key.as_deref() == Some(key));
        assert!(!has("sidebar-empty-new-session"), "no New Terminal button");
        assert!(!has("sidebar-empty-new-session-dropdown"));
        assert!(!has("sidebar-add-project"), "no Add Project button");
        assert!(has("sidebar-projects-new-session"), "the + is there");
        assert!(
            has("sidebar-workspace-selector"),
            "the workspace selector is there"
        );
    }

    /// Both sidebar pickers are rune-kit's searchable select, opened by their
    /// triggers and as wide as the sidebar's content.
    #[test]
    fn the_plus_and_workspace_pickers_open_a_full_width_search() {
        use rune::{Cx, Env, Runtime};

        fn view(s: &State, _: &mut Cx<State>) -> El {
            let mut layer = Vec::new();
            let bar = sidebar(s, &s.ink, &mut layer);
            new_session_layer(s, &mut layer);
            rune_kit::root(rune::row().child(bar), layer)
        }

        for picker in ["new-session", "sidebar-workspace"] {
            let mut s = State::new(test_handle());
            s.data.agents = vec![crate::data::AgentInfo {
                id: "shell".into(),
                label: "Shell".into(),
                available: true,
                path: None,
                binary: None,
            }];
            let width = s.sidebar_width;
            match picker {
                "new-session" => {
                    let width = picker_width(&s);
                    open_new_session(&mut s, PROJECTS_HEADER_KEY.to_string(), None, width);
                }
                _ => s.workspace_picker.open = true,
            }
            let mut rt = Runtime::new(s, view, (900.0, 700.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let els = rt.elements();
            let surface = els
                .iter()
                .find(|e| e.key.as_deref() == Some(&format!("{picker}-content")))
                .unwrap_or_else(|| panic!("{picker}: its surface is open"));
            let [x, _, w, _] = surface.rect;
            assert!(
                x >= 0.0 && x + w <= width,
                "{picker}: the surface stays over the sidebar ({x}..{} of {width})",
                x + w
            );
            assert!(
                w >= width - 24.0,
                "{picker}: the surface spans the sidebar ({w} of {width})"
            );
            let search = els
                .iter()
                .find(|e| e.key.as_deref() == Some("search") && e.role == "textbox")
                .unwrap_or_else(|| panic!("{picker}: its search field is open"));
            assert!(
                search.rect[2] >= w - 8.5,
                "{picker}: the search spans the surface"
            );
        }
    }

    /// With a project active in the selected workspace, `+` → Shell starts in
    /// its checkout, so the new tab stays inside the project filter.
    #[test]
    fn plus_shell_starts_in_the_active_project_of_this_workspace() {
        let mut s = State::new(test_handle());
        s.editor.projects = vec![app_core::projects::ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: "web".into(),
                root: "/work/a/web".into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }];
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        start_picked_session(&mut s, "shell");
        assert_eq!(s.view, crate::View::Terminal);
        let launch = s
            .pending
            .iter()
            .find_map(|p| match p {
                crate::sessions::Pending::Create { launch, .. } => Some(launch),
                _ => None,
            })
            .expect("a shell launch went out");
        assert_eq!(launch.project_id.as_deref(), Some("p1"));
        assert_eq!(launch.cwd, "/work/a/web");
    }

    /// In a new workspace (the active project lives elsewhere), `+` → Shell
    /// starts a terminal that belongs to this workspace: no project, and the
    /// workspace recorded, so its row and tab show here.
    #[test]
    fn plus_shell_in_a_new_workspace_belongs_to_it() {
        let mut s = State::new(test_handle());
        s.workspaces = vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", ""),
        ];
        s.editor.projects = vec![app_core::projects::ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: "web".into(),
                root: "/work/a/web".into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }];
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.selected_workspace = Some("w1".into());
        start_picked_session(&mut s, "shell");
        let (launch, workspace_id) = s
            .pending
            .iter()
            .find_map(|p| match p {
                crate::sessions::Pending::Create {
                    launch,
                    workspace_id,
                    ..
                } => Some((launch, workspace_id)),
                _ => None,
            })
            .expect("a shell launch went out");
        assert_eq!(launch.project_id, None, "not the other workspace's project");
        assert_eq!(workspace_id.as_deref(), Some("w1"));
    }
}

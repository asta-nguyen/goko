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
}

/// What a row's action button opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// The agents a session can be started with.
    NewSession,
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
        // The menu opens where the pointer is: the button remembers the
        // last position it saw.
        .on_pointer_move(|s: &mut State, p: rune::PointerPress| {
            s.menu_anchor = Some((p.x, p.y));
        })
        .on_press(action)
}

/// The actions a project or worktree row offers: a session to start and
/// the row's other commands.
fn checkout_actions(ink: &Ink, target: RowTarget) -> El {
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
            "New session",
            format!("sidebar-action:new-session:{identity}"),
            move |s: &mut State| {
                s.open_menu(new_session.clone(), MenuKind::NewSession);
            },
        ))
        .child(action_button(
            ink,
            &theme::MORE,
            "More actions",
            format!("sidebar-action:more:{identity}"),
            move |s: &mut State| {
                s.open_menu(more.clone(), MenuKind::More);
            },
        ))
}

/// The actions a session row offers: stop it and close its pane.
fn session_actions(ink: &Ink, session_id: String) -> El {
    let stop = session_id.clone();
    let close = session_id.clone();
    row()
        .w(ACTIONS_W)
        .h(ROW_H)
        .gap(2.0)
        .items_center()
        .child(action_button(
            ink,
            &theme::SQUARE,
            "Stop session",
            format!("sidebar-action:stop:{session_id}"),
            move |s: &mut State| {
                s.stop_session(&stop, false);
            },
        ))
        .child(action_button(
            ink,
            &theme::X,
            "Close pane",
            format!("sidebar-action:close:{session_id}"),
            move |s: &mut State| {
                s.delete_session(&close);
            },
        ))
}

/// The open menu, as a kit layer. `None` when no menu is open.
pub fn menu_layer(s: &State, layer: &mut Vec<El>) -> Option<El> {
    let (target, kind) = s.sidebar_menu.clone()?;
    let (x, y) = s.menu_anchor.unwrap_or((0.0, 0.0));
    let mut menu = rune_kit::Menu::new("sidebar-menu").at(x, y).open(true);
    match kind {
        MenuKind::NewSession => {
            for agent in &s.data.agents {
                let agent_id = agent.id.clone();
                let label = agent.label.clone();
                let available = agent.available;
                let target = target.clone();
                menu = menu.item(MenuItem::new(label).disabled(!available).on_select_tx(
                    move |st: &mut State, _tx: &mut Tx| {
                        st.sidebar_menu = None;
                        st.request_session_argv(target.clone(), agent_id.clone());
                    },
                ));
            }
            menu = menu.item(MenuItem::new("Custom command…").on_select_tx(
                move |st: &mut State, _tx: &mut Tx| {
                    st.sidebar_menu = None;
                    st.ask_for_input(SidebarInput::CustomCommand {
                        target: target.clone(),
                    });
                },
            ));
        }
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
                        RowTarget::Session { .. } => (String::new(), None),
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
                RowTarget::Session { .. } => String::new(),
            };
            if !project_id.is_empty() {
                let pid = project_id.clone();
                menu = menu.item(MenuItem::new("Refresh worktrees").on_select_tx(
                    move |st: &mut State, _tx: &mut Tx| {
                        st.sidebar_menu = None;
                        st.editor.request_worktrees(&pid);
                    },
                ));
                let pid = project_id.clone();
                menu = menu.item(MenuItem::new("Create worktree").on_select_tx(
                    move |st: &mut State, _tx: &mut Tx| {
                        st.sidebar_menu = None;
                        st.ask_for_input(SidebarInput::NewWorktree {
                            project_id: pid.clone(),
                        });
                    },
                ));
            }
            match &target {
                RowTarget::Worktree { project_id, path } => {
                    let pid = project_id.clone();
                    let path = path.clone();
                    menu = menu.item(MenuItem::new("Remove worktree").danger(true).on_select_tx(
                        move |st: &mut State, _tx: &mut Tx| {
                            st.sidebar_menu = None;
                            st.remove_worktree(&pid, &path);
                        },
                    ));
                }
                RowTarget::Project { project_id } => {
                    let pid = project_id.clone();
                    menu = menu.item(MenuItem::new("Remove project").danger(true).on_select_tx(
                        move |st: &mut State, _tx: &mut Tx| {
                            st.sidebar_menu = None;
                            st.remove_project(&pid);
                        },
                    ));
                }
                RowTarget::Session { .. } => {}
            }
        }
    }
    Some(
        menu.on_close(|st: &mut State| st.sidebar_menu = None)
            .build(&s.editor.kit, layer),
    )
}

/// The sidebar's text prompt, as a kit layer: the command a custom session
/// runs, or the name of a new worktree.
pub fn input_layer(s: &State, ink: &Ink, layer: &mut Vec<El>) -> Option<El> {
    let input = s.sidebar_input.clone()?;
    let (placeholder, title) = match &input {
        SidebarInput::CustomCommand { .. } => ("Command to run…", "New session"),
        SidebarInput::NewWorktree { .. } => ("Worktree name…", "New worktree"),
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
            let input = s.sidebar_input.take();
            s.sidebar_text = rune::edit::TextEdit::new("");
            match input {
                Some(SidebarInput::CustomCommand { target }) => {
                    s.launch_custom_session(target, &text);
                }
                Some(SidebarInput::NewWorktree { project_id }) => {
                    s.create_worktree(&project_id, &text);
                }
                None => {}
            }
            true
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
const ROW_H: f32 = 24.0; // every row's pitch: project, worktree, session, "Worktrees"
// Filled rows (active, selected) keep a 2 pt gap so neighbouring fills never merge into
// one blob. The fill is an inset `row_layer` (`FILL_H` tall, `ROW_GAP` in from the top
// and bottom) rather than a margin on the row, so every row keeps its exact 24 pt pitch
// and the text/badges inside never move off the pixel grid.
const ROW_GAP: f32 = 1.0;
const FILL_H: f32 = ROW_H - 2.0 * ROW_GAP; // 22
const CELL: f32 = 20.0; // chevron cell (was 28)
const GUIDE: f32 = 13.0; // indent guide offset (was 16); ≈ under the chevron centre
const SESSION_PL: f32 = 20.0; // session icon indent inside its list (was 40)
const NAME: TextStyle = TextStyle::new(13.0, 500.0).lh(16.0); // was T14M
const SUB: TextStyle = T12.lh(16.0); // session title (was T12)
const BRANCH: TextStyle = T11.lh(16.0); // inline branch (was T11, on its own line)
// Header / meta text on the same 16 pt line box, so it sits on whole pixels in its row.
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
fn add_project(ink: &Ink) -> El {
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
        .on_press_tx(|_s: &mut State, tx: &mut Tx| {
            tx.open_folder(
                rune::file_dialog::FileDialogOptions {
                    title: "Choose a project folder".into(),
                    ..Default::default()
                },
                |st: &mut State, result: rune::file_dialog::FileDialogResult, _tx: &mut Tx| {
                    if let Ok(Some(path)) = result {
                        st.editor
                            .backend
                            .send(Job::project(Req::AddProject(path.display().to_string())));
                    }
                },
            );
        })
}

/// Disclosure chevron. The glyph, not a box, takes the hover (muted → text), so a
/// hovered row never carries a second hover surface inside it; the 20 px cell stays
/// the hit target and shrinks a touch while pressed. Toggling stays instant: this is
/// used many times a day, so motion would only slow it down.
fn chevron(open: bool, ink: &Ink) -> El {
    el().size(CELL, CELL)
        .center()
        .hover_group()
        .press_scale(0.9)
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
/// directly on each other still read as two.
fn row_layer(fill: Color) -> El {
    el().absolute(0.0, ROW_GAP)
        .w_full()
        .h(FILL_H)
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
pub fn sidebar(s: &State, ink: &Ink) -> El {
    let known: HashSet<&str> = s
        .editor
        .projects
        .iter()
        .map(|p| p.project.id.as_str())
        .collect();
    let (by_project, unassigned) = data::partition(&s.data, &known);

    let empty: &[&SessionView] = &[];
    let mut list = col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .gap(4.0)
        .pr(8.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if s.editor.projects.is_empty() {
        // Loading / error / empty are distinct honest states — the list
        // never fabricates a project row.
        let (text, tone) = if s.editor.projects_loading {
            ("Loading projects…".to_string(), ink.muted)
        } else if let Some(err) = &s.editor.projects_error {
            (err.clone(), ink.danger)
        } else {
            (
                "No projects registered. Add a project folder to start.".to_string(),
                ink.muted,
            )
        };
        list = list.child(status_row(&text, tone));
    } else if let Some(err) = &s.editor.projects_error {
        list = list.child(status_row(err, ink.danger));
    }
    for project in &s.editor.projects {
        let sessions = by_project
            .get(project.project.id.as_str())
            .map_or(empty, Vec::as_slice);
        list = list.child(project_node(s, ink, project, sessions));
    }
    if !unassigned.is_empty() {
        list = list.child(unassigned_node(s, ink, &unassigned));
    }

    col().w(s.sidebar_width).h_full().fill(ink.panel).child(
        col()
            .w_full()
            .h_full()
            .min_h(0.0)
            .gap(4.0)
            .padding(8.0)
            // header row `px-1`: h2 + ghost-sm "Add Project"
            .child(
                row()
                    .w_full()
                    .h(ROW_H)
                    .px(4.0)
                    .items_center()
                    .justify_between()
                    .child(theme::txt("PROJECTS", HDR, ink.muted))
                    .child(add_project(ink)),
            )
            .child(list),
    )
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
        .child(theme::txt(text, T12, tone).min_w(0.0).truncate())
}

fn project_node(s: &State, ink: &Ink, project: &ProjectView, sessions: &[&SessionView]) -> El {
    let pid = project.project.id.as_str();
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

    // Row: `flex items-center gap-1 rounded-md px-1 py-0.5` (gap 2, px 4, h 24).
    let mut row_el = row()
        .w_full()
        .h(ROW_H)
        .px(4.0)
        .gap(2.0)
        .items_center()
        .radius(6.0)
        .hover_group()
        // The whole row is one press target, so it lights up as one surface (`press`
        // only paints on the element that owns the press handler). The chevron keeps
        // its own handler and captures its own presses.
        .press(press_wash(ink))
        .child(row_layer(row_fill(ink, active)).child(active_edge(ink, active)));

    // Chevron button 20×20; expanded chevron-down, collapsed chevron-right.
    let pid_toggle = pid.to_string();
    row_el = row_el.child(chevron(expanded, ink).on_press(move |s: &mut State| {
        let cur = s.expanded.get(pid_toggle.as_str()).copied().unwrap_or(true);
        s.expanded.insert(pid_toggle.clone(), !cur);
    }));

    // Label cell: `flex-1 min-w-0 items-center gap-1.5 px-1 py-1.5` (gap 6, px 4,
    // no vertical padding). The TSX's `active` → bg-secondary/70 was panel-on-panel
    // and invisible, and its press state only covered this cell; the fill, edge and
    // press now belong to the whole `row_el`.
    let mut activate = row().grow(1.0).min_w(0.0).px(4.0).gap(6.0).items_center();
    activate = activate
        .child(theme::ic(
            if is_git {
                &theme::FOLDER_GIT2
            } else {
                &theme::FOLDER
            },
            16.0,
            ink.accent,
        ))
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .gap(6.0)
                .items_center()
                .child(
                    theme::txt(&project.project.name, NAME, ink.text)
                        .group_color(lit(ink))
                        .min_w(40.0)
                        .truncate(),
                )
                .children(checkout_status(project.missing, root, ink))
                .children(branch_subtitle(is_git, root, ink)),
        );
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
    row_el = row_el.key(format!("sidebar-project:{pid}")).on_press_tx(
        move |s: &mut State, tx: &mut Tx| {
            navigation::navigate(
                s,
                PendingNav::SelectCheckout {
                    project_id: pid_select.clone(),
                    checkout: root_checkout.clone(),
                },
                tx,
            );
        },
    );
    // The REVEAL hover-only action menus are opacity-0 by default — omitted,
    // matching the real app's resting render.
    row_el = row_el.child(checkout_actions(
        ink,
        RowTarget::Project {
            project_id: pid.to_string(),
        },
    ));

    let mut node = col().child(row_el);
    // A failed worktree read for THIS project surfaces under its row —
    // same honest-error placement the desktop gives `worktreeQuery.error`.
    if let Some(err) = entry.and_then(|e| e.error.as_ref()) {
        node = node.child(
            row()
                .ml(GUIDE + 6.0)
                .px(8.0)
                .h(ROW_H)
                .items_center()
                .child(theme::txt(err, T11M, ink.danger).min_w(0.0).truncate()),
        );
    }
    if expanded {
        if !root_sessions.is_empty() {
            node = node.child(session_items(s, ink, &root_sessions, &workspace_id, true));
        }
        if let Some(group) = worktree_group(s, ink, pid, worktrees, sessions) {
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
        Some(true) => Some(el().size(6.0, 6.0).radius(3.0).fill(ink.warning)),
        None => Some(el().size(6.0, 6.0).radius(3.0).fill(ink.muted)),
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
            .truncate(),
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
    // showLabel: a label exists and differs from the title (case-insensitive).
    let label = sv
        .agent_label(&s.data.agents)
        .filter(|l| !l.eq_ignore_ascii_case(title.as_str()));

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
        .press(press);
    b = if indent {
        b.pl(SESSION_PL).pr(8.0)
    } else {
        b.px(8.0)
    };
    // bg-accent/10 + text-accent on the row; the fill is always present (alpha 0 when
    // not selected) so selecting is a colour transition, not a fill appearing.
    b = b.child(row_layer(ink.accent.alpha(if selected {
        0.10
    } else {
        0.0
    })));
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
    b = b.child(agent_badge(sv, ink, badge_bg, s.spinner_angle()));
    // The row's own actions: stop the session and close its pane. They sit
    // in a fixed cell, so the row never shifts when they appear.
    b = b.child(session_actions(ink, info.id.clone()));
    if let Some(l) = label {
        // label font-medium (sel→accent / ended→muted / else text) + title
        // `text-xs text-muted` truncate.
        b = b
            .child(theme::txt(l, NAME, rest).group_color(hovered))
            .child(
                theme::txt(&title, SUB, ink.muted)
                    .group_color(ink.text)
                    .min_w(0.0)
                    .truncate(),
            );
    } else {
        // single title font-medium, same colour rule.
        b = b.child(
            theme::txt(&title, NAME, rest)
                .group_color(hovered)
                .min_w(0.0)
                .truncate(),
        );
    }
    col().child(b)
}

/// The 16×16 icon cell: AgentIcon plus the StateDot overlay — a 12 px disc in
/// `row_bg` (panel, or the selected row's accent tint, so no halo shows through)
/// under an 8 px dot, both centred at (14,14)
/// (`-bottom-0.5 -right-0.5 size-2.5 ring-2`).
fn agent_badge(s: &SessionView, ink: &Ink, row_bg: Color, angle: f32) -> El {
    let (glyph, color) = theme::agent_glyph(s.agent_id(), ink);
    el().size(16.0, 16.0)
        .child(theme::ic(glyph, 16.0, color))
        // The state dot sits over the mark's corner, on a disc of the row's
        // own background so no glyph shows through it.
        .child(
            el().absolute(6.0, 6.0)
                .size(10.0, 10.0)
                .center()
                .child(el().size(8.0, 8.0).radius(4.0).fill(row_bg))
                .child(theme::state_dot(s.info.state, angle, ink)),
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
                            .truncate(),
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
            .truncate(),
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
                .truncate(),
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
        .child(row_layer(row_fill(ink, active)).child(active_edge(ink, active)))
        .child(chevron(wt_open, ink).on_press(move |s: &mut State| {
            let cur = s
                .worktrees_open
                .get(wt_path.as_str())
                .copied()
                .unwrap_or(true);
            s.worktrees_open.insert(wt_path.clone(), !cur);
        }))
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
        .child(row_layer(row_fill(ink, unassigned_active)))
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
}

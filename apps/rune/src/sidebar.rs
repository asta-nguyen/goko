//! The project sidebar — a port of `features/projects/ProjectTree.tsx` plus the
//! nested `SessionItems`, `WorktreeRows`, `StateDot`/`StateBadge`, `AgentIcon`,
//! `CheckoutStatus` and `Count` pieces it composes. Metrics follow the TSX where
//! fidelity.rs deviated (8 px state dots, TONE/DOT colour maps, the real
//! label-collapse and other-sessions nesting quirks). Row states (hover / press /
//! active / selected) deliberately deviate from the TSX, which painted them
//! panel-on-panel and invisible; the TSX was fixed the same way. The sidebar uses
//! a compact 4-pt-grid scale (see the constants below), deliberately denser than
//! the TSX.

use rune::{col, el, row, spacer, Color, El, ScrollSpec, TextStyle};

use crate::data::{self, Project, SessionView, WorktreeInfo};
use crate::theme::{self, Ink, T11, T11M, T11ST, T12, T128, T12TN};
use crate::State;

const SIDEBAR_W: f32 = 320.0; // PANEL_LIMITS.projectTree.defaultWidth

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
const PCT: TextStyle = T12TN.lh(16.0); // session context %

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
fn add_project(ink: &Ink) -> El {
    row()
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
pub fn sidebar(s: &State, ink: &Ink) -> El {
    let (by_project, unassigned) = data::partition(&s.data);

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
    for project in &s.data.projects {
        let sessions = by_project
            .get(project.id.as_str())
            .map_or(empty, Vec::as_slice);
        list = list.child(project_node(s, ink, project, sessions));
    }
    if !unassigned.is_empty() {
        list = list.child(unassigned_node(s, ink, &unassigned));
    }

    col().w(SIDEBAR_W).h_full().fill(ink.panel).child(
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
/// full height.
pub fn separator(ink: &Ink) -> El {
    el().w(8.0)
        .h_full()
        .child(el().absolute(3.5, 0.0).w(1.0).h_full().fill(ink.border))
}

// ---------------------------------------------------------------------------
// ProjectNode
// ---------------------------------------------------------------------------

fn project_node(s: &State, ink: &Ink, project: &Project, sessions: &[&SessionView]) -> El {
    let expanded = s.expanded.get(project.id.as_str()).copied().unwrap_or(true);
    let wt_list = s.data.worktrees.get(project.id.as_str());
    let is_git = wt_list.is_some_and(|w| w.repo);
    let worktrees: &[WorktreeInfo] = wt_list.map_or(&[], |w| w.worktrees.as_slice());
    let root = worktrees.iter().find(|w| w.is_root);
    let root_path = root.map_or(project.root.as_str(), |r| r.path.as_str());

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

    let workspace_id = data::checkout_key(&project.id, &project.root);
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
    let pid_toggle = project.id.clone();
    row_el = row_el.child(chevron(expanded, ink).on_press(move |s: &mut State| {
        let cur = s.expanded.get(pid_toggle.as_str()).copied().unwrap_or(true);
        s.expanded.insert(pid_toggle.clone(), !cur);
    }));

    // Label cell: `flex-1 min-w-0 items-center gap-1.5 px-1 py-1.5` (gap 6, px 4,
    // no vertical padding). The TSX's `active` → bg-secondary/70 was panel-on-panel
    // and invisible, and its press state only covered this cell; the fill, edge and
    // press now belong to the whole `row_el`.
    let pid_active = workspace_id.clone();
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
                    theme::txt(&project.name, NAME, ink.text)
                        .group_color(lit(ink))
                        .min_w(40.0)
                        .truncate(),
                )
                .children(checkout_status(project, root, ink))
                .children(branch_subtitle(is_git, root, ink)),
        );
    row_el = row_el.child(activate);

    // Collapsed && sessions>0 → Count badge.
    if !expanded && !sessions.is_empty() {
        // `mr(4)`: the row's own px(4) sits inside the label cell's px(4) for every
        // other right-hand element, so insets match (badge, context %, group badges).
        row_el = row_el.child(theme::count_badge(sessions.len(), ink).mr(4.0));
    }
    row_el = row_el.on_press(move |s: &mut State| {
        s.active_workspace = Some(pid_active.clone());
    });
    // The REVEAL hover-only action menus are opacity-0 by default — omitted,
    // matching the real app's resting render.

    let mut node = col().child(row_el);
    if expanded {
        if !root_sessions.is_empty() {
            node = node.child(session_items(s, ink, &root_sessions, &workspace_id, true));
        }
        if let Some(group) = worktree_group(s, ink, project, worktrees, sessions) {
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
fn checkout_status(project: &Project, root: Option<&WorktreeInfo>, ink: &Ink) -> Option<El> {
    if project.missing {
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
    let tab_title = title.clone();
    b = b.on_press(move |s: &mut State| {
        s.selected_session = Some(sid.clone());
        s.active_workspace = Some(wid.clone());
        // Activating a session activates its workspace tab, opening one when
        // the title isn't open yet — like TerminalWorkspace's focus path.
        match s.open_tabs.iter().position(|t| *t == tab_title) {
            Some(i) => s.active_tab = i,
            None => {
                s.open_tabs.push(tab_title.clone());
                s.active_tab = s.open_tabs.len() - 1;
            }
        }
    });

    let badge_bg = if selected {
        ink.panel.mix(ink.accent, 0.10)
    } else {
        ink.panel
    };
    b = b.child(agent_badge(sv, ink, badge_bg));
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
    if let Some(pct) = sv.context_pct {
        // usageTone(): >=90 danger, >=80 warning, else muted; ml-auto tnum.
        let tone = if pct >= 90.0 {
            ink.danger
        } else if pct >= 80.0 {
            ink.warning
        } else {
            ink.muted
        };
        b = b
            .child(spacer())
            .child(theme::txt(&format!("{}%", pct.round() as u8), PCT, tone));
    }
    col().child(b)
}

/// The 16×16 icon cell: AgentIcon plus the StateDot overlay — a 12 px disc in
/// `row_bg` (panel, or the selected row's accent tint, so no halo shows through)
/// under an 8 px dot, both centred at (14,14)
/// (`-bottom-0.5 -right-0.5 size-2.5 ring-2`).
fn agent_badge(sv: &SessionView, ink: &Ink, row_bg: Color) -> El {
    let (glyph, color) = theme::agent_glyph(sv.agent_id(), ink);
    let dot = sv.info.state.dot_color(sv.activity, ink);
    el().size(16.0, 16.0)
        .child(theme::ic(glyph, 16.0, color))
        .child(
            el().absolute(8.0, 8.0)
                .size(12.0, 12.0)
                .radius(6.0)
                .fill(row_bg),
        )
        .child(
            el().absolute(10.0, 10.0)
                .size(8.0, 8.0)
                .radius(4.0)
                .fill(dot),
        )
}

// ---------------------------------------------------------------------------
// WorktreeRows
// ---------------------------------------------------------------------------

/// `null` unless the project is a git repo with at least one linked worktree.
fn worktree_group(
    s: &State,
    ink: &Ink,
    project: &Project,
    worktrees: &[WorktreeInfo],
    sessions: &[&SessionView],
) -> Option<El> {
    let linked: Vec<&WorktreeInfo> = worktrees.iter().filter(|w| !w.is_root).collect();
    if linked.is_empty() {
        return None;
    }
    let group_open = s
        .worktree_groups
        .get(project.id.as_str())
        .copied()
        .unwrap_or(true);
    // Outer ul metrics; the whole header row is one toggle button.
    let mut outer = col()
        .ml(GUIDE)
        .pl(6.0)
        .gap(0.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], ink.border);
    let pid_group = project.id.clone();
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
        for wt in linked {
            inner = inner.child(worktree_node(s, ink, project, wt, worktrees, sessions));
        }
        outer = outer.child(inner);
    }
    Some(outer)
}

fn worktree_node(
    s: &State,
    ink: &Ink,
    project: &Project,
    wt: &WorktreeInfo,
    worktrees: &[WorktreeInfo],
    sessions: &[&SessionView],
) -> El {
    let checkout_sessions = data::sessions_for_checkout(worktrees, sessions, &wt.path);
    let workspace_id = data::checkout_key(&project.id, &wt.path);
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

    let wid = workspace_id.clone();
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
    row_el = row_el.on_press(move |s: &mut State| {
        s.active_workspace = Some(wid.clone());
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
        });
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

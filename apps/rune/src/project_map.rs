//! Global workspace/project navigation over the registered project cache.

use rune::edit::TextEdit;
use rune::{El, Key, KeyEvent, ScrollSpec, Tx, col, el, row};

use crate::editor::navigation::{self, PendingNav};
use crate::{State, View, theme};

const SCROLL: &str = "project-map-rows";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Workspace(String),
    Project(String),
}

impl Target {
    fn key(&self) -> String {
        match self {
            Self::Workspace(id) => format!("project-map-workspace:{id}"),
            Self::Project(id) => format!("project-map-project:{id}"),
        }
    }
}

struct MapRow {
    target: Target,
    name: String,
    detail: String,
    current: bool,
}

#[derive(Default)]
pub struct ProjectMapState {
    pub open: bool,
    pub query: TextEdit,
    cursor: Option<Target>,
    return_focus: Option<String>,
}

fn rows(s: &State) -> Vec<MapRow> {
    let query = s.project_map.query.text.trim().to_lowercase();
    let mut rows = Vec::new();
    for workspace in &s.workspaces {
        let label = s.workspace_label(workspace);
        let all = query.is_empty() || label.to_lowercase().contains(&query);
        let projects: Vec<_> = s
            .editor
            .projects
            .iter()
            .filter(|p| {
                p.project.workspace_id == workspace.id
                    && (all
                        || p.project.name.to_lowercase().contains(&query)
                        || p.project.root.to_lowercase().contains(&query))
            })
            .collect();
        if !all && projects.is_empty() {
            continue;
        }
        rows.push(MapRow {
            target: Target::Workspace(workspace.id.clone()),
            name: label,
            detail: if projects.is_empty() {
                "No projects".into()
            } else {
                format!("{} projects", projects.len())
            },
            current: workspace.id == s.workspace_id(),
        });
        for project in projects {
            let p = &project.project;
            rows.push(MapRow {
                target: Target::Project(p.id.clone()),
                name: p.name.clone(),
                detail: p.root.clone(),
                current: s.editor.checkout_key.is_some()
                    && s.editor.project_id.as_deref() == Some(&p.id),
            });
        }
    }
    rows
}

fn ready(s: &State) -> bool {
    !s.workspaces.is_empty()
        && !s.workspaces_loading
        && !s.editor.projects_loading
        && s.workspaces_error.is_none()
        && s.editor.projects_error.is_none()
}

pub fn available(s: &State) -> bool {
    s.view != View::Settings
        && s.sidebar_input.is_none()
        && s.sidebar_confirm.is_none()
        && s.workspace_remove_confirm.is_none()
        && s.sidebar_menu.is_none()
        && !s.sidebar_picker_open()
        && s.usage_menu.is_none()
        && s.editor.worktree_form.is_none()
        && s.editor.guard.is_none()
        && s.editor.save_resume.is_none()
        && !s.editor.quick_open.open
        && !s.editor.project_picker.open
        && !s.editor.checkout_picker.open
        && !s.editor.history.branch_picker.open
        && !s.editor.commit.agent_picker.open
        && s.editor.history.row_menu.is_none()
        && (s.view != View::Terminal || s.preedit.is_empty())
}

pub fn open(s: &mut State, tx: &mut Tx) {
    if s.project_map.open || !available(s) {
        return;
    }
    s.project_map = ProjectMapState {
        open: true,
        query: TextEdit::new(""),
        return_focus: tx.focused().map(str::to_string),
        ..Default::default()
    };
    s.project_map.query.focused = true;
    poll(s);
}

pub fn close(s: &mut State) {
    s.project_map = ProjectMapState::default();
}

/// Keep the cursor on the same identity across membership/read replies.
pub fn poll(s: &mut State) -> bool {
    if !s.project_map.open {
        return false;
    }
    let rows = rows(s);
    if s.project_map
        .cursor
        .as_ref()
        .is_some_and(|id| rows.iter().any(|r| &r.target == id))
    {
        return false;
    }
    let next = rows.first().map(|r| r.target.clone());
    if s.project_map.cursor == next {
        return false;
    }
    s.project_map.cursor = next;
    true
}

fn activate(s: &mut State, target: Target, tx: &mut Tx) {
    if !ready(s) {
        return;
    }
    close(s);
    match target {
        Target::Workspace(id) => {
            if s.workspaces.iter().any(|w| w.id == id) {
                s.select_workspace(&id);
            } else {
                s.notice = Some("The workspace is no longer available".into());
            }
        }
        Target::Project(project_id) => {
            if !s.editor.projects.iter().any(|p| {
                p.project.id == project_id
                    && s.workspaces.iter().any(|w| w.id == p.project.workspace_id)
            }) {
                s.notice = Some("The project or its workspace is no longer available".into());
                return;
            }
            navigation::navigate(s, PendingNav::SelectMappedProject { project_id }, tx);
        }
    }
}

fn command(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Escape => {
            close(s);
            true
        }
        Key::Up | Key::Down => {
            let rows = rows(s);
            if rows.is_empty() {
                return true;
            }
            let at = s
                .project_map
                .cursor
                .as_ref()
                .and_then(|id| rows.iter().position(|r| &r.target == id))
                .unwrap_or(0);
            let next = if ev.key == Key::Down {
                (at + 1).min(rows.len() - 1)
            } else {
                at.saturating_sub(1)
            };
            let target = rows[next].target.clone();
            tx.scroll_into_view_in(SCROLL, target.key());
            s.project_map.cursor = Some(target);
            true
        }
        Key::Enter => {
            if let Some(target) = s.project_map.cursor.clone() {
                activate(s, target, tx);
            }
            true
        }
        _ => false,
    }
}

fn system_key(ev: &KeyEvent) -> bool {
    ev.mods.cmd && matches!(ev.key, Key::Command('w' | 'q'))
}

/// Field keys run before TextEdit; let ordinary editing and Tab stay native.
fn input_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if command(s, ev, tx) {
        return true;
    }
    if system_key(ev) {
        return false;
    }
    ev.mods.cmd && matches!(ev.key, Key::Command('t' | ',' | 'k' | '1'..='9'))
}

/// Unhandled input must never reach the editor or a terminal behind the map.
pub fn key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if command(s, ev, tx) {
        return true;
    }
    !system_key(ev) && ev.key != Key::Tab
}

pub fn layers(s: &State) -> Vec<El> {
    if !s.project_map.open {
        return Vec::new();
    }
    let ink = &s.ink;
    let kit = &s.editor.kit;
    let rows = rows(s);
    let enabled = ready(s);
    let input = rune_kit::TextInput::new(&s.project_map.query)
        .label("Find projects")
        .placeholder("Workspace, project or path…")
        .width(None)
        .on_edit(|s: &mut State| &mut s.project_map.query)
        .on_key(input_key)
        .on_change(|s: &mut State, tx: &mut Tx| {
            s.project_map.cursor = None;
            poll(s);
            if let Some(target) = &s.project_map.cursor {
                tx.scroll_into_view_in(SCROLL, target.key());
            }
        })
        .build(kit);
    let close_button = rune_kit::Button::new("Close")
        .on_press(|s: &mut State| close(s))
        .build(kit);
    let mut results = col()
        .key(SCROLL)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(4.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for r in &rows {
        let is_project = matches!(r.target, Target::Project(_));
        let target = r.target.clone();
        let cursor = s.project_map.cursor.as_ref() == Some(&r.target);
        let label = format!(
            "{}: {}, {}{}",
            if is_project { "Project" } else { "Workspace" },
            r.name,
            r.detail,
            if r.current { ", current" } else { "" }
        );
        let mut line = row()
            .key(r.target.key())
            .w_full()
            .min_h(42.0)
            .pl(if is_project { 24.0 } else { 8.0 })
            .pr(8.0)
            .py(6.0)
            .gap(8.0)
            .items_center()
            .radius(4.0)
            .role("button")
            .label(label.clone())
            .help(label)
            .disabled(!enabled)
            .hover(ink.text.alpha(0.06))
            .press(ink.text.alpha(0.12))
            .on_press_tx(move |s: &mut State, tx: &mut Tx| activate(s, target.clone(), tx))
            .child(theme::txt(
                if is_project { "└" } else { "▾" },
                theme::T12,
                ink.muted,
            ))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child({
                        theme::txt(&r.name, theme::T12M, ink.text)
                            .truncate_middle()
                            .help_when_truncated(r.name.clone())
                    })
                    .child(
                        theme::txt(&r.detail, theme::T11, ink.muted)
                            .truncate_middle()
                            .help_when_truncated(r.detail.clone()),
                    ),
            );
        if r.current {
            line = line.child(theme::txt("✓ Current", theme::T11, ink.accent));
        }
        if cursor {
            line = line.fill(ink.text.alpha(0.10));
        }
        results = results.child(line);
    }
    let status = if s.workspaces_loading || s.editor.projects_loading {
        "Loading projects and workspaces…".to_string()
    } else if let Some(error) = s
        .workspaces_error
        .as_ref()
        .or(s.editor.projects_error.as_ref())
    {
        error.clone()
    } else if s.workspaces.is_empty() {
        "Workspaces are not available".into()
    } else if rows.is_empty() {
        "No matching projects or workspaces".into()
    } else if s.editor.projects.is_empty() {
        "No registered projects".into()
    } else {
        "↑↓ select · Enter switch · Esc close".into()
    };
    let mut card = col()
        .key("project-map-dialog")
        .role("dialog")
        .label("Project Map")
        .w_full()
        .max_w(660.0)
        .h_full()
        .max_h(560.0)
        .min_h(0.0)
        .fill(ink.panel)
        .border_sides([1.0; 4], ink.border)
        .radius(8.0)
        .padding(12.0)
        .gap(8.0)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(theme::txt("Project Map", theme::T12M, ink.text))
                .child(close_button),
        )
        .child(input)
        .child(theme::txt(
            &status,
            theme::T11,
            if s.workspaces_error.is_some() || s.editor.projects_error.is_some() {
                ink.danger
            } else {
                ink.muted
            },
        ))
        .child(results)
        .modal(true, close)
        .modal_click_outside(true);
    if s.workspaces_error.is_some() || s.editor.projects_error.is_some() {
        card = card.child(
            rune_kit::Button::new("Retry")
                .on_press(|s: &mut State| {
                    s.request_workspaces();
                    s.editor.refresh_projects();
                })
                .build(kit),
        );
    }
    if let Some(target) = &s.project_map.cursor {
        card = card.active_descendant(target.key());
    }
    if let Some(key) = &s.project_map.return_focus {
        card = card.focus_return(key);
    }
    vec![
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(ink.bg.alpha(0.55))
            .a11y_hidden(),
        col()
            .key("project-map-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(20.0)
            .items_center()
            .justify_center()
            .child(card),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::{
        projects::ProjectView,
        store::{Project, Workspace},
    };

    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        s.workspaces = [("default", "Default"), ("work", "Work"), ("empty", "Empty")]
            .into_iter()
            .map(|(id, name)| Workspace {
                id: id.into(),
                name: name.into(),
                created_at: 0,
            })
            .collect();
        s.editor.projects = [("a", "default", "/work/ALPHA"), ("b", "work", "/work/Beta")]
            .into_iter()
            .map(|(id, workspace, path)| ProjectView {
                project: Project {
                    id: id.into(),
                    name: "Same name".into(),
                    root: path.into(),
                    workspace_id: workspace.into(),
                    created_at: 0,
                },
                missing: false,
            })
            .collect();
        s
    }

    #[test]
    fn project_map_hierarchy_and_filter_keep_stable_duplicate_ids() {
        let mut s = scene();
        assert_eq!(rows(&s).len(), 5);
        assert_eq!(rows(&s)[1].target, Target::Project("a".into()));
        assert_eq!(rows(&s)[3].target, Target::Project("b".into()));
        for (query, count) in [
            ("   ", 5),
            ("same NAME", 4),
            ("ALpha", 2),
            ("Default", 2),
            ("EMPTY", 1),
            ("nothing", 0),
        ] {
            s.project_map.query.text = query.into();
            assert_eq!(rows(&s).len(), count, "{query}");
        }
        s.project_map.query.text = "work".into();
        assert_eq!(
            rows(&s).len(),
            4,
            "workspace match includes its children; path matches keep parent"
        );
    }

    #[test]
    fn project_map_cursor_follows_identity_across_replies_and_queries() {
        let mut s = scene();
        open(&mut s, &mut Tx::default());
        s.project_map.cursor = Some(Target::Project("b".into()));
        s.editor.projects[1].project.workspace_id = "default".into();
        poll(&mut s);
        assert_eq!(s.project_map.cursor, Some(Target::Project("b".into())));
        s.editor.projects.pop();
        assert!(poll(&mut s));
        assert_eq!(
            s.project_map.cursor,
            Some(Target::Workspace("default".into()))
        );
        s.project_map.query.text = "no matches".into();
        poll(&mut s);
        assert_eq!(s.project_map.cursor, None);
    }

    #[test]
    fn project_map_unready_data_cannot_activate() {
        for condition in 0..4 {
            let mut s = scene();
            open(&mut s, &mut Tx::default());
            match condition {
                0 => s.workspaces_loading = true,
                1 => s.editor.projects_loading = true,
                2 => s.workspaces_error = Some("database busy".into()),
                _ => s.editor.projects_error = Some("projects failed".into()),
            }
            activate(&mut s, Target::Project("b".into()), &mut Tx::default());
            assert!(s.project_map.open);
            assert!(s.editor.project_id.is_none());
        }
    }

    #[test]
    fn project_map_workspace_click_preserves_checkout_and_stale_id_is_rejected() {
        let mut s = scene();
        s.editor.project_id = Some("a".into());
        s.editor.checkout = Some("/work/ALPHA".into());
        open(&mut s, &mut Tx::default());
        activate(&mut s, Target::Workspace("work".into()), &mut Tx::default());
        assert_eq!(s.workspace_id(), "work");
        assert_eq!(s.editor.project_id.as_deref(), Some("a"));
        assert_eq!(s.editor.checkout, Some("/work/ALPHA".into()));
        open(&mut s, &mut Tx::default());
        activate(&mut s, Target::Workspace("gone".into()), &mut Tx::default());
        assert_eq!(s.workspace_id(), "work");
        assert!(s.notice.is_some());
    }
    #[test]
    fn project_map_removed_project_is_rejected_before_opening_a_draft_guard() {
        let mut s = scene();
        let mut doc = crate::editor::draft::OpenDoc::new(
            "notes.txt",
            &app_core::git_view::GitViewFile {
                path: "notes.txt".into(),
                kind: "text",
                text: Some("original".into()),
                truncated: false,
                size: 8,
            },
        );
        doc.code.edit.text = "unsaved".into();
        s.editor.doc = Some(doc);
        open(&mut s, &mut Tx::default());
        s.editor.projects.retain(|p| p.project.id != "b");
        activate(&mut s, Target::Project("b".into()), &mut Tx::default());
        assert!(!s.project_map.open);
        assert!(s.editor.guard.is_none());
        assert!(s.editor.project_id.is_none());
        assert!(s.editor.doc.as_ref().unwrap().dirty());
        assert!(s.notice.is_some());
    }
}

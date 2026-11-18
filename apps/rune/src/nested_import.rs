//! Add Project's nested-repository review: scan a folder, choose repositories,
//! then register them under the folder, separately, or open the folder alone.

use rune::edit::TextEdit;
use rune::{El, Key, KeyEvent, ScrollSpec, Tx, col, el, row};

use crate::editor::backend::{Job, Req};
use crate::{State, theme};

const SCROLL: &str = "nested-import-rows";

#[derive(Default)]
pub struct NestedImportState {
    pub open: bool,
    pub token: u64,
    pub path: String,
    pub parent_id: Option<String>,
    pub scanning: bool,
    pub scan: Option<app_core::project_scan::NestedScanResult>,
    pub selected: std::collections::HashSet<String>,
    pub cursor: usize,
    pub group_name: TextEdit,
    pub importing: bool,
    pub error: Option<String>,
    pub return_focus: Option<String>,
}

pub fn close(s: &mut State) {
    s.nested_import = NestedImportState::default();
}

/// Ask for a project folder, then scan it. Used by both Add Project entry
/// points.
pub fn start_add_project(s: &mut State, tx: &mut Tx) {
    if s.nested_import.open {
        return;
    }
    let focus = tx.focused().map(str::to_string);
    tx.open_folder(
        rune::file_dialog::FileDialogOptions {
            title: "Choose a project folder".into(),
            ..Default::default()
        },
        move |st: &mut State, result: rune::file_dialog::FileDialogResult, _tx: &mut Tx| {
            match result {
                Ok(Some(path)) => start_scan(st, path.display().to_string(), None, focus.clone()),
                Ok(None) => {}
                Err(error) => st.notice = Some(error.to_string()),
            }
        },
    );
}

/// Rescan a registered folder project from its sidebar row.
#[allow(dead_code)]
pub fn start_rescan(s: &mut State, tx: &mut Tx, parent_id: String, path: String) {
    if s.nested_import.open {
        return;
    }
    let focus = tx.focused().map(str::to_string);
    start_scan(s, path, Some(parent_id), focus);
}

fn start_scan(s: &mut State, path: String, parent_id: Option<String>, focus: Option<String>) {
    let token = s.nested_import_token;
    s.nested_import_token += 1;
    s.nested_import = NestedImportState {
        open: true,
        token,
        path: path.clone(),
        parent_id,
        scanning: true,
        return_focus: focus,
        ..Default::default()
    };
    if !s
        .editor
        .backend
        .send(Job::project(Req::ScanNestedRepos { path, token }))
    {
        s.nested_import.scanning = false;
        s.nested_import.error = Some("The editor queue is full — try again".into());
    }
}

/// Apply a scan reply; only the visible dialog's token counts.
pub fn apply_scan(s: &mut State, token: u64, result: app_core::project_scan::NestedScanResult) {
    if !s.nested_import.open || s.nested_import.token != token {
        return;
    }
    s.nested_import.scanning = false;
    s.nested_import.error = None;
    if result.selected_path_kind == app_core::project_scan::SelectedPathKind::GitRepo {
        let path = result.selected_path.clone();
        close(s);
        if !s.editor.backend.send(Job::project(Req::AddProject {
            path,
            workspace_id: s.workspace_id().to_string(),
        })) {
            s.notice = Some("The editor queue is full — try again".into());
        }
        return;
    }
    let name = std::path::Path::new(&result.selected_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    s.nested_import.group_name = TextEdit::new(name);
    s.nested_import.selected = result.repos.iter().map(|repo| repo.path.clone()).collect();
    s.nested_import.cursor = 0;
    s.nested_import.scan = Some(result);
}

/// Apply a failed scan/import reply; only the visible dialog's token counts.
pub fn apply_error(s: &mut State, token: u64, error: String) {
    if !s.nested_import.open || s.nested_import.token != token {
        return;
    }
    s.nested_import.scanning = false;
    s.nested_import.importing = false;
    s.nested_import.error = Some(error);
}

/// Apply an import reply; only the visible dialog's token counts.
pub fn apply_import(s: &mut State, token: u64, result: app_core::projects::NestedImportResult) {
    if !s.nested_import.open || s.nested_import.token != token {
        return;
    }
    s.nested_import.importing = false;
    let parent = result.parent.clone();
    let imported: Vec<_> = result
        .projects
        .iter()
        .filter_map(|entry| entry.project.clone())
        .collect();
    let target = parent
        .as_ref()
        .map(|project| project.project.id.clone())
        .or_else(|| imported.first().map(|project| project.project.id.clone()));
    let counts = format!(
        "{} imported, {} already known, {} failed",
        result.imported, result.already_known, result.failed
    );
    close(s);
    s.notice = Some(format!("Repositories: {counts}"));
    for project in parent.iter().chain(imported.iter()) {
        s.join_sessions_to_project(project);
    }
    s.editor.refresh_projects();
    s.nested_import_select = target;
}

fn submit(s: &mut State, mode: app_core::projects::ImportMode) {
    let Some(scan) = s.nested_import.scan.as_ref() else {
        return;
    };
    let paths: Vec<String> = scan
        .repos
        .iter()
        .filter(|repo| s.nested_import.selected.contains(&repo.path))
        .map(|repo| repo.path.clone())
        .collect();
    if mode != app_core::projects::ImportMode::Folder && paths.is_empty() {
        s.nested_import.error = Some("Select at least one repository, or open the folder.".into());
        return;
    }
    let token = s.nested_import.token;
    let parent_path = scan.selected_path.clone();
    let parent_id = s.nested_import.parent_id.clone();
    let group_name = s.nested_import.group_name.text.trim().to_string();
    let workspace_id = s.workspace_id().to_string();
    s.nested_import.importing = true;
    s.nested_import.error = None;
    if !s.editor.backend.send(Job::project(Req::ImportNestedRepos {
        parent_path,
        parent_id,
        group_name,
        project_paths: paths,
        mode,
        workspace_id,
        token,
    })) {
        s.nested_import.importing = false;
        s.nested_import.error = Some("The editor queue is full — try again".into());
    }
}

fn toggle(s: &mut State, path: &str) {
    if !s.nested_import.selected.remove(path) {
        s.nested_import.selected.insert(path.to_string());
    }
}

fn move_cursor(s: &mut State, down: bool) {
    let Some(scan) = &s.nested_import.scan else {
        return;
    };
    if scan.repos.is_empty() {
        return;
    }
    let at = s.nested_import.cursor.min(scan.repos.len() - 1);
    s.nested_import.cursor = if down {
        (at + 1).min(scan.repos.len() - 1)
    } else {
        at.saturating_sub(1)
    };
}

pub fn key(s: &mut State, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    if !s.nested_import.open {
        return false;
    }
    if ev.mods.cmd && matches!(ev.key, Key::Command('w' | 'q')) {
        return false;
    }
    match &ev.key {
        Key::Escape => {
            close(s);
            true
        }
        Key::Enter if !ev.mods.cmd && !ev.mods.ctrl => {
            submit(s, app_core::projects::ImportMode::Group);
            true
        }
        Key::Text(text) if text == " " && !ev.mods.cmd && !ev.mods.ctrl => {
            let path = s
                .nested_import
                .scan
                .as_ref()
                .and_then(|scan| scan.repos.get(s.nested_import.cursor))
                .map(|repo| repo.path.clone());
            if let Some(path) = path {
                toggle(s, &path);
            }
            true
        }
        Key::Up => {
            move_cursor(s, false);
            true
        }
        Key::Down => {
            move_cursor(s, true);
            true
        }
        _ => true,
    }
}

pub fn layers(s: &State) -> Vec<El> {
    if !s.nested_import.open {
        return Vec::new();
    }
    let ink = &s.ink;
    let kit = &s.editor.kit;
    let d = &s.nested_import;
    let scanning = d.scanning || d.importing;
    let close_button = rune_kit::Button::new("Cancel")
        .on_press(|s: &mut State| close(s))
        .build(kit);
    let mut card = col()
        .key("nested-import-dialog")
        .role("dialog")
        .label("Add project")
        .w_full()
        .max_w(620.0)
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
                .child(theme::txt("Add project", theme::T12M, ink.text))
                .child(close_button),
        )
        .child(
            theme::txt(&d.path, theme::T11, ink.muted)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(d.path.clone()),
        );
    if let Some(error) = &d.error {
        card = card.child(theme::txt(error, theme::T11, ink.danger));
    }
    if d.scanning {
        card = card.child(theme::txt("Scanning repositories…", theme::T12, ink.muted));
    } else if let Some(scan) = &d.scan {
        if scan.repos.is_empty() {
            card = card.child(theme::txt(
                "No Git repositories found in this folder.",
                theme::T12,
                ink.muted,
            ));
            card = card.child(
                row().w_full().justify_end().gap(8.0).child(
                    rune_kit::Button::new("Open as Folder")
                        .on_press(|s: &mut State| submit(s, app_core::projects::ImportMode::Folder))
                        .build(kit),
                ),
            );
        } else {
            card = card.child(theme::txt(
                &format!("Found {} repositories.", scan.repos.len()),
                theme::T11,
                ink.muted,
            ));
            let mut list = col()
                .key(SCROLL)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(2.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                });
            for (index, repo) in scan.repos.iter().enumerate() {
                let path = repo.path.clone();
                let selected = d.selected.contains(&repo.path);
                let cursor = d.cursor == index;
                let checkbox = rune_kit::Checkbox::new(selected)
                    .label(repo.display_name.clone())
                    .description(repo.path.clone())
                    .on_toggle(move |s: &mut State| toggle(s, &path))
                    .build(kit);
                let mut line = row()
                    .key(format!("nested-import-repo:{}", repo.path))
                    .w_full()
                    .min_h(34.0)
                    .pl(8.0 + (repo.depth.saturating_sub(1) as f32) * 14.0)
                    .pr(8.0)
                    .items_center()
                    .radius(4.0)
                    .child(checkbox);
                if cursor {
                    line = line.fill(ink.text.alpha(0.08));
                }
                list = list.child(line);
            }
            card = card.child(list);
            card = card.child(
                rune_kit::TextInput::new(&d.group_name)
                    .label("Group name")
                    .width(None)
                    .on_edit(|s: &mut State| &mut s.nested_import.group_name)
                    .build(kit),
            );
            card = card.child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(8.0)
                    .child(
                        rune_kit::Button::new("Import separately")
                            .on_press(|s: &mut State| {
                                submit(s, app_core::projects::ImportMode::Separate)
                            })
                            .build(kit),
                    )
                    .child(
                        rune_kit::Button::new("Import as group")
                            .on_press(|s: &mut State| {
                                submit(s, app_core::projects::ImportMode::Group)
                            })
                            .build(kit),
                    ),
            );
        }
        if scan.truncated || scan.timed_out || scan.stopped {
            card = card.child(theme::txt(
                "The scan stopped at its limit; some repositories may be missing.",
                theme::T11,
                ink.muted,
            ));
        }
    }
    if scanning {
        card = card.child(theme::txt("Working…", theme::T11, ink.muted));
    }
    let mut card = card.modal(true, close).modal_click_outside(true);
    if let Some(key) = &d.return_focus {
        card = card.focus_return(key);
    }
    vec![
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(ink.bg.alpha(0.55))
            .a11y_hidden(),
        col()
            .key("nested-import-layer")
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

    fn state() -> State {
        State::new(crate::test_handle())
    }

    #[test]
    fn a_scan_selects_every_candidate_and_names_the_group() {
        let mut s = state();
        s.nested_import_token = 4;
        s.nested_import = NestedImportState {
            open: true,
            token: 4,
            path: "/tmp/parent".into(),
            scanning: true,
            ..Default::default()
        };
        let result = app_core::project_scan::NestedScanResult {
            selected_path: "/tmp/parent".into(),
            selected_path_kind: app_core::project_scan::SelectedPathKind::NonGitFolder,
            repos: vec![
                app_core::project_scan::NestedRepoCandidate {
                    path: "/tmp/parent/a".into(),
                    display_name: "a".into(),
                    depth: 1,
                },
                app_core::project_scan::NestedRepoCandidate {
                    path: "/tmp/parent/b".into(),
                    display_name: "b".into(),
                    depth: 1,
                },
            ],
            truncated: false,
            timed_out: false,
            stopped: false,
            duration_ms: 1,
            max_depth: 3,
            max_repos: 100,
            max_dirs: 10_000,
            timeout_ms: 5_000,
        };
        apply_scan(&mut s, 4, result);
        assert!(!s.nested_import.scanning);
        assert_eq!(s.nested_import.selected.len(), 2);
        assert_eq!(s.nested_import.group_name.text, "parent");
    }

    #[test]
    fn a_stale_token_is_dropped() {
        let mut s = state();
        s.nested_import = NestedImportState {
            open: true,
            token: 2,
            scanning: true,
            ..Default::default()
        };
        let result = app_core::project_scan::NestedScanResult {
            selected_path: "/tmp/parent".into(),
            selected_path_kind: app_core::project_scan::SelectedPathKind::NonGitFolder,
            repos: Vec::new(),
            truncated: false,
            timed_out: false,
            stopped: false,
            duration_ms: 1,
            max_depth: 3,
            max_repos: 100,
            max_dirs: 10_000,
            timeout_ms: 5_000,
        };
        apply_scan(&mut s, 1, result);
        assert!(s.nested_import.scan.is_none());
        assert!(s.nested_import.scanning);
    }

    #[test]
    fn space_toggles_the_cursor_row_and_escape_closes() {
        let mut s = state();
        s.nested_import = NestedImportState {
            open: true,
            token: 1,
            scan: Some(app_core::project_scan::NestedScanResult {
                selected_path: "/tmp/parent".into(),
                selected_path_kind: app_core::project_scan::SelectedPathKind::NonGitFolder,
                repos: vec![app_core::project_scan::NestedRepoCandidate {
                    path: "/tmp/parent/a".into(),
                    display_name: "a".into(),
                    depth: 1,
                }],
                truncated: false,
                timed_out: false,
                stopped: false,
                duration_ms: 1,
                max_depth: 3,
                max_repos: 100,
                max_dirs: 10_000,
                timeout_ms: 5_000,
            }),
            selected: ["/tmp/parent/a".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let mut tx = Tx::default();
        assert!(key(
            &mut s,
            &KeyEvent {
                key: Key::Text(" ".into()),
                mods: rune::Mods::default()
            },
            &mut tx
        ));
        assert!(s.nested_import.selected.is_empty());
        assert!(key(
            &mut s,
            &KeyEvent {
                key: Key::Escape,
                mods: rune::Mods::default()
            },
            &mut tx
        ));
        assert!(!s.nested_import.open);
    }
}

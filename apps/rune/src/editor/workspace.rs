//! The editor workspace chrome: project/checkout pickers, the four-section
//! tab strip, the stale/error notice, the draft-guard dialog, and dispatch
//! into the mounted section views. Sections keep their own scroll/selection
//! state so switching back is instant, like the desktop's visited set.
//!
//! rune-kit overlays (open `Select` menus) collect into a per-frame `Vec`
//! passed to `rune_kit::root`, matching the kit-gallery pattern.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use app_core::projects::ProjectView;
use app_core::worktrees::WorktreeList;
use rune::{El, Tx, col, row};
use rune_kit::Kit;

use crate::State;
use crate::editor::backend::{self, Req};
use crate::editor::navigation::{self, GuardAction, PendingNav};
use crate::editor::state::Section;
use crate::theme::{self, Ink};

/// Picker entries are `(label, stable identity)` pairs: rune-kit `Select`
/// matches on the label string alone, so the label must be unique AND carry
/// enough information to recover the identity. Labels stay human-readable —
/// a collision appends the path (then the id as a last resort) instead of
/// changing every option's appearance.
fn disambiguate(entries: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for (label, _) in &entries {
        *counts.entry(label.clone()).or_default() += 1;
    }
    let mut used = HashSet::new();
    entries
        .into_iter()
        .map(|(label, id)| {
            let mut label = label;
            if counts.get(label.as_str()).copied().unwrap_or(0) > 1 || used.contains(&label) {
                // Second uniqueness tier: caller folded the path into the
                // base label already when names collided; `id` is the
                // guaranteed-unique last resort.
                if used.contains(&label) {
                    label = format!("{label} · {id}");
                }
            }
            used.insert(label.clone());
            (label, id)
        })
        .collect()
}

/// Registered projects → `(label, project_id)`. Same-name projects get the
/// root path folded into the label so each stays individually selectable.
fn project_options(projects: &[ProjectView]) -> Vec<(String, String)> {
    let mut name_count = std::collections::HashMap::<&str, usize>::new();
    for p in projects {
        *name_count.entry(p.project.name.as_str()).or_default() += 1;
    }
    let entries = projects
        .iter()
        .map(|p| {
            let label = if name_count[p.project.name.as_str()] > 1 {
                format!("{} — {}", p.project.name, p.project.root)
            } else {
                p.project.name.clone()
            };
            (label, p.project.id.clone())
        })
        .collect();
    disambiguate(entries)
}

/// Project root + listed worktrees → `(label, checkout path)`. The main
/// worktree IS the project root: it appears once as "root" and any listed
/// entry that duplicates it (by `is_root` or equal path) is skipped.
/// Same-name worktrees get their path folded into the label.
fn checkout_options(root: Option<&str>, worktrees: &WorktreeList) -> Vec<(String, PathBuf)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(root) = root {
        entries.push(("root".to_string(), root.to_string()));
    }
    for wt in &worktrees.worktrees {
        if wt.missing || wt.is_root {
            continue;
        }
        let path = PathBuf::from(&wt.path);
        if root.is_some_and(|r| Path::new(r) == path.as_path()) {
            continue; // the root worktree must not appear twice
        }
        entries.push((wt.name.clone(), wt.path.clone()));
    }
    let mut name_count = std::collections::HashMap::<String, usize>::new();
    for (label, _) in &entries {
        *name_count.entry(label.clone()).or_default() += 1;
    }
    disambiguate(
        entries
            .into_iter()
            .map(|(label, path)| {
                let label = if name_count.get(label.as_str()).copied().unwrap_or(0) > 1 {
                    format!("{label} — {path}")
                } else {
                    label
                };
                (label, path)
            })
            .collect(),
    )
    .into_iter()
    .map(|(label, path)| (label, PathBuf::from(path)))
    .collect()
}

/// Root of the editor view.
pub fn workspace(s: &State, ink: &Ink, k: &Kit) -> El {
    let mut layer = Vec::new();
    let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0);
    root = root.child(pickers(s, ink, k, &mut layer));
    if s.editor.checkout.is_some() {
        root = root.child(section_tabs(s, ink));
        if let Some(notice) = &s.editor.notice {
            root = root.child(notice_bar(notice, ink));
        }
        root = root.child(section_body(s, ink, k).grow(1.0).min_h(0.0));
    } else {
        root = root.child(empty_state(s, ink));
    }
    // Guard dialog renders above everything (Save/Discard/Cancel) as a real
    // modal: `Dialog` + `El::modal` trap focus, inert the background and
    // answer Escape — all required for the dirty-draft guard to be honest.
    if s.editor.guard.is_some() {
        guard_dialog(s, ink, k, &mut layer);
    }
    // `kit::root` wraps the base in an `h_full` div: without `grow` it claims
    // the whole column and pushes the section content below the tab/controls
    // bars (the buried-footer defect). `grow(1).min_h(0)` makes it take the
    // space the bars leave instead.
    rune_kit::root(root, layer).grow(1.0).min_h(0.0)
}

/// Refresh the reads the active section needs — the entry point
/// `navigation::set_section`/`refresh_checkout_data` call.
pub fn refresh_active(s: &mut State) {
    s.editor.refresh_active();
}

/// Project picker + checkout picker + "add project" — the only place editor
/// checkout selection happens; it never reads the mock terminal session.
fn pickers(s: &State, ink: &Ink, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut bar = row()
        .w_full()
        .min_h(36.0)
        .flex_wrap()
        .px(12.0)
        .gap(8.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .fill(ink.panel);

    // Project select (native rune-kit Select — fully controlled: `open`
    // mirrors `editor.project_picker.open`, toggled/dismissed via the
    // callbacks; choosing routes through the draft guard as SelectProject
    // so a Cancel keeps the old project AND checkout intact). Options carry
    // unique labels mapped to project ids — same-name projects stay
    // individually selectable.
    let project_opts = project_options(&s.editor.projects);
    let options: Vec<String> = project_opts.iter().map(|(l, _)| l.clone()).collect();
    let current = s
        .editor
        .project_id
        .as_deref()
        .and_then(|id| project_opts.iter().find(|(_, pid)| pid == id))
        .map(|(l, _)| l.clone());
    // The field trigger has a fixed width — a path-disambiguated project name
    // must not stretch the bar. `.shrink()` lets it give way at narrow
    // widths; the label truncates and `.help` keeps the full identity.
    let project_help = current.clone().unwrap_or_else(|| "Select a project".into());
    bar = bar.child(
        rune_kit::Combobox::new(
            "editor-project",
            &s.editor.project_picker,
            &crate::picker::options(&options),
            |st: &mut State| &mut st.editor.project_picker,
        )
        .current(current.as_deref())
        .placeholder("Select a project")
        .label("Project")
        .search_placeholder("Search projects")
        .width(200.0)
        .popover_width(crate::picker::list_width(200.0))
        .on_choose(move |st: &mut State, label: &str, tx: &mut Tx| {
            if let Some((_, pid)) = project_opts.iter().find(|(l, _)| l == label) {
                navigation::navigate(
                    st,
                    PendingNav::SelectProject {
                        project_id: pid.clone(),
                    },
                    tx,
                );
            }
        })
        .help(project_help)
        .build(k, layer)
        .shrink()
        .w(90.0)
        .grow(1.0)
        .max_w(200.0)
        .min_w(90.0),
    );

    // Checkout select: project root + listed worktrees of the selected
    // project. Options map unique labels to canonical checkout paths; the
    // root worktree appears once and same-name worktrees disambiguate by
    // path, so every entry selects its intended identity.
    let project_root = s.editor.project_id.as_deref().and_then(|pid| {
        s.editor
            .projects
            .iter()
            .find(|p| p.project.id == pid)
            .map(|p| p.project.root.as_str())
    });
    let checkout_opts = checkout_options(project_root, &s.editor.worktrees);
    let labels: Vec<String> = checkout_opts.iter().map(|(l, _)| l.clone()).collect();
    let current_checkout = s.editor.checkout.as_ref().and_then(|c| {
        checkout_opts
            .iter()
            .find(|(_, p)| p.as_path() == c.as_path())
            .map(|(l, _)| l.clone())
    });
    let checkout_help = current_checkout
        .clone()
        .unwrap_or_else(|| "Select a checkout".into());
    bar = bar.child(
        rune_kit::Combobox::new(
            "editor-checkout",
            &s.editor.checkout_picker,
            &crate::picker::options(&labels),
            |st: &mut State| &mut st.editor.checkout_picker,
        )
        .current(current_checkout.as_deref())
        .placeholder(if s.editor.worktrees_loading {
            "Loading…"
        } else {
            "Select a checkout"
        })
        .label("Checkout")
        .search_placeholder("Search checkouts")
        .width(240.0)
        .popover_width(crate::picker::list_width(240.0))
        .on_choose(move |st: &mut State, label: &str, tx: &mut Tx| {
            let Some(pid) = st.editor.project_id.clone() else {
                return;
            };
            let Some((_, path)) = checkout_opts.iter().find(|(l, _)| l == label) else {
                return;
            };
            navigation::navigate(
                st,
                PendingNav::SelectCheckout {
                    project_id: pid,
                    checkout: path.clone(),
                },
                tx,
            );
        })
        .help(checkout_help)
        .build(k, layer)
        .shrink()
        .w(90.0)
        .grow(1.0)
        .max_w(240.0)
        .min_w(90.0),
    );

    if let Some(co) = &s.editor.checkout {
        bar = bar.child(crate::editor::branch::view(&s.editor, ink));
        bar = bar.child(
            row()
                .w(0.0)
                .grow(1.0)
                .min_w(0.0)
                .shrink()
                .gap(6.0)
                .items_center()
                .px(8.0)
                .h(26.0)
                .radius(6.0)
                .fill(ink.panel)
                .border(1.0, ink.border)
                .help(co.display().to_string())
                .child(theme::ic(&theme::FOLDER, 12.0, ink.muted))
                .child(
                    theme::txt(&co.display().to_string(), theme::T11M, ink.muted)
                        .min_w(0.0)
                        .truncate_middle(),
                ),
        );
    }

    if let Some(err) = &s.editor.projects_error {
        bar = bar.child(theme::txt(err, theme::T11M_LH, ink.danger));
    }
    if let Some(err) = &s.editor.worktrees_error {
        bar = bar.child(theme::txt(err, theme::T11M_LH, ink.danger));
    }
    col().w_full().child(bar)
}

/// The four section tabs; the default section comes from state (Changes,
/// matching the desktop workspace).
fn section_tabs(s: &State, ink: &Ink) -> El {
    let mut tabs = row()
        .h(36.0)
        .w_full()
        .px(8.0)
        .gap(4.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        // A focus group gives the tabs arrow-key roving and reports each
        // tab's selected state to assistive tech (ARIA tab pattern).
        .focus_group();
    for section in Section::ALL {
        let active = s.editor.section == section;
        tabs = tabs.child(el_tab(section, active, ink).on_press_tx(
            move |st: &mut State, tx: &mut Tx| {
                navigation::navigate(st, PendingNav::SetSection(section), tx);
            },
        ));
    }
    tabs
}

fn el_tab(section: Section, active: bool, ink: &Ink) -> rune::El {
    use rune::el;
    let tone = if active { ink.text } else { ink.muted };
    let hover_tone = ink.text;
    let mut b = el()
        .key(format!("section:{}", section.label()))
        .h(28.0)
        .px(12.0)
        .radius(7.0)
        .items_center()
        .justify_center()
        .hover_group()
        .hover(if active {
            rune::Color::TRANSPARENT
        } else {
            ink.text.alpha(0.06)
        })
        .press_scale(0.96)
        .motion(theme::STATE_SPRING)
        .role("tab")
        .label(section.label())
        // `Role::Tab` is a selectable item: this reports selected/unselected
        // both in `Runtime::elements` (`item_selected`) and the OS a11y tree.
        .focus_selected(active)
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent);
    if active {
        b = b.child(
            el().key("thumb")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .radius(7.0)
                .fill(ink.text.alpha(0.10))
                .matched("workspace-section-thumb"),
        );
    }
    b.child(
        theme::txt(section.label(), theme::T12M, tone)
            .key("label")
            .group_color(hover_tone),
    )
}

fn notice_bar(notice: &crate::editor::state::Notice, ink: &Ink) -> El {
    let tone = if notice.stale {
        ink.warning
    } else {
        ink.danger
    };
    row()
        .w_full()
        .px(12.0)
        .py(6.0)
        .gap(8.0)
        .items_center()
        .fill(tone.alpha(0.08))
        .border_sides([0.0, 0.0, 1.0, 0.0], tone.alpha(0.25))
        .child(theme::txt(&notice.text, theme::T12, tone))
        .child(
            theme::btn_sm("Dismiss notice", None, true, ink).on_press(|st: &mut State| {
                st.editor.notice = None;
            }),
        )
}

/// Dispatch to the active section's `view(&State)` entry — sibling workers
/// own `changes`/`history`/`stashes` (+ `commit`/`patch`/`commit_detail`
/// panes they compose). Selection/ensure work that needs `&mut State` runs
/// from reply handlers and `navigation::set_section`, never the render pass.
fn section_body(s: &State, ink: &Ink, k: &Kit) -> El {
    let _ = (ink, k);
    match s.editor.section {
        Section::Files => crate::editor::files::files_view(s, ink, k),
        Section::Changes => crate::editor::changes::view(s),
        Section::History => crate::editor::history::view(s),
        Section::Stashes => crate::editor::stashes::view(s),
        Section::Servers => crate::editor::lsp::servers_view(s, ink, k),
    }
}

/// One pressable project in the empty Editor: its name and root path. The
/// press is the sidebar project row's own draft-guarded root selection.
fn project_row(project: &ProjectView, ink: &Ink) -> El {
    let pid = project.project.id.clone();
    let root = project.project.root.clone();
    let checkout = PathBuf::from(&project.project.root);
    col()
        .w_full()
        .px(10.0)
        .py(6.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .role("button")
        .label(format!("Open {}", project.project.name))
        .help(root.clone())
        .focusable(true)
        .key(format!("editor-empty-project:{}", project.project.id))
        .child({
            theme::txt(&project.project.name, theme::T12M, ink.text)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(project.project.name.clone())
        })
        .child({
            theme::txt(&root, theme::T11, ink.muted)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(root.clone())
        })
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            navigation::navigate(
                s,
                PendingNav::SelectCheckout {
                    project_id: pid.clone(),
                    checkout: checkout.clone(),
                },
                tx,
            );
        })
}

fn empty_state(s: &State, ink: &Ink) -> El {
    let body = if s.editor.projects.is_empty() && !s.editor.projects_loading {
        "No projects registered. Add a project folder to start editing."
    } else if s.editor.projects.is_empty() {
        "Loading projects…"
    } else {
        "Select a project and checkout to open the editor."
    };
    let mut column = col()
        .grow(1.0)
        .w_full()
        .items_center()
        .justify_center()
        .gap(8.0)
        .child(theme::txt("Editor", theme::T14M, ink.text))
        .child(theme::txt(body, theme::T12, ink.muted));
    // Up to five registered projects, each opening its root checkout.
    if !s.editor.projects.is_empty() {
        let mut list = col().w_full().max_w(320.0).gap(6.0);
        for project in s.editor.projects.iter().take(5) {
            list = list.child(project_row(project, ink));
        }
        column = column.child(list);
    }
    column.child(
        theme::btn_sm("Add project", Some((&theme::FOLDER_PLUS, 14.0)), true, ink).on_press_tx(
            |_st: &mut State, tx: &mut Tx| {
                tx.open_folder(
                    rune::file_dialog::FileDialogOptions {
                        title: "Choose a project folder".into(),
                        ..Default::default()
                    },
                    |st: &mut State, result: rune::file_dialog::FileDialogResult, _tx: &mut Tx| {
                        if let Ok(Some(path)) = result {
                            st.editor
                                .backend
                                .send(backend::Job::project(Req::AddProject {
                                    path: path.display().to_string(),
                                    workspace_id: st.workspace_id().to_string(),
                                }));
                        }
                    },
                );
            },
        ),
    )
}

/// The dirty-draft guard as a rune-kit `Dialog`: `role="dialog"`, a trapped
/// focus scope, an inert background and Escape/outside-press → Cancel. The
/// kit's `on_close` has no `Tx`, so dismissal routes through
/// `navigation::cancel_guard` (identical to `resolve_guard(Cancel)`).
fn guard_dialog(s: &State, ink: &Ink, k: &Kit, layer: &mut Vec<El>) {
    let quitting = s.editor.guard.as_ref().is_some_and(|g| g.quitting);
    let title = if quitting {
        "Save changes before quitting?"
    } else {
        "Save changes before leaving?"
    };
    let buttons = row()
        .gap(8.0)
        .justify_end()
        .w_full()
        .child(theme::btn_sm("Cancel", None, true, ink).on_press_tx(
            |st: &mut State, tx: &mut Tx| {
                navigation::resolve_guard(st, GuardAction::Cancel, tx);
            },
        ))
        .child(theme::btn_sm("Discard", None, true, ink).on_press_tx(
            |st: &mut State, tx: &mut Tx| {
                if st.editor.quit_vetoed {
                    navigation::quit_discard(st);
                } else {
                    navigation::resolve_guard(st, GuardAction::Discard, tx);
                }
            },
        ))
        .child(theme::btn_sm("Save", None, false, ink).on_press_tx(
            |st: &mut State, tx: &mut Tx| {
                if st.editor.quit_vetoed {
                    navigation::quit_after_save(st);
                } else {
                    navigation::resolve_guard(st, GuardAction::Save, tx);
                }
            },
        ));
    rune_kit::Dialog::new("editor-guard", title)
        .open(true)
        .fit()
        .width(400.0)
        .description("The open file has unsaved edits.")
        .on_close(|st: &mut State| navigation::cancel_guard(st))
        .child(buttons)
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::projects::ProjectView;
    use app_core::store::Project;
    use app_core::worktrees::WorktreeInfo;

    fn project(id: &str, name: &str, root: &str) -> ProjectView {
        ProjectView {
            project: Project {
                id: id.into(),
                name: name.into(),
                root: root.into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }
    }

    fn worktree(name: &str, path: &str, is_root: bool) -> WorktreeInfo {
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

    fn wt_list(worktrees: Vec<WorktreeInfo>) -> WorktreeList {
        WorktreeList {
            repo: true,
            worktrees,
        }
    }

    #[test]
    fn current_branch_header_survives_every_section_and_narrow_layout() {
        use rune::{Env, Runtime};
        let root = format!("/repo/{}", "long-directory/".repeat(12));
        let branch_name = format!("feature/{}", "long-branch-name-".repeat(12));
        let mut s = State::new(crate::test_handle());
        s.view = crate::View::Editor;
        s.editor.projects = vec![project("p", "A project", &root)];
        s.editor.project_id = Some("p".into());
        s.editor.checkout = Some(PathBuf::from(&root));
        s.editor.checkout_key = Some("current".into());
        s.editor.branch.checkout = Some("current".into());
        s.editor.branch.data = Some(crate::editor::branch::Branch {
            name: Some(branch_name.clone()),
            head: "12345678".into(),
        });
        let mut rt = Runtime::new(
            s,
            |s, _| workspace(s, &Ink::new(), &s.editor.kit),
            (480.0, 700.0),
            Env::new(true, 2.0),
        );
        for section in [
            Section::Files,
            Section::Changes,
            Section::History,
            Section::Stashes,
            Section::Servers,
        ] {
            rt.update(|s, _| s.editor.section = section);
            rt.frame(0.0);
            let header = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("editor-current-branch") && !e.exiting)
                .unwrap();
            assert_eq!(header.label, Some(format!("Current branch: {branch_name}")));
            assert!(header.rect[2] >= 176.0 && header.rect[2] <= 240.0);
            assert!(
                header.rect[0] >= 0.0 && header.rect[0] + header.rect[2] <= 480.1,
                "{section:?}: {:?}",
                header.rect
            );
        }
    }

    #[test]
    fn same_name_projects_get_distinct_selectable_ids() {
        let projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "web", "/work/b/web"),
            project("p3", "api", "/work/c/api"),
        ];
        let opts = project_options(&projects);
        assert_eq!(opts.len(), 3);
        let labels: Vec<&str> = opts.iter().map(|(l, _)| l.as_str()).collect();
        // Unique labels — Select can match each one back to its id.
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
        // The two "web" projects disambiguate by root; "api" stays clean.
        assert!(labels.contains(&"api"));
        assert!(labels.iter().any(|l| l.starts_with("web — /work/a/web")));
        assert!(labels.iter().any(|l| l.starts_with("web — /work/b/web")));
        // Each label resolves to exactly one intended id.
        let id_of = |label: &str| {
            opts.iter()
                .find(|(l, _)| l == label)
                .map(|(_, i)| i.as_str())
        };
        assert_eq!(
            id_of(labels.iter().find(|l| l.contains("/work/a/web")).unwrap()),
            Some("p1")
        );
        assert_eq!(
            id_of(labels.iter().find(|l| l.contains("/work/b/web")).unwrap()),
            Some("p2")
        );
    }

    #[test]
    fn identical_name_and_root_projects_still_disambiguate() {
        let projects = vec![
            project("p1", "web", "/same/root"),
            project("p2", "web", "/same/root"),
        ];
        let opts = project_options(&projects);
        let labels: Vec<&str> = opts.iter().map(|(l, _)| l.as_str()).collect();
        assert_ne!(labels[0], labels[1]);
        assert_eq!(opts[0].1, "p1");
        assert_eq!(opts[1].1, "p2");
    }

    #[test]
    fn same_name_worktrees_get_distinct_paths() {
        let wts = wt_list(vec![
            worktree("feature", "/repo/wt-a", false),
            worktree("feature", "/repo/wt-b", false),
            worktree("dev", "/repo/wt-c", false),
        ]);
        let opts = checkout_options(Some("/repo"), &wts);
        assert_eq!(opts.len(), 4); // root + 3 worktrees
        let labels: Vec<&str> = opts.iter().map(|(l, _)| l.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
        assert!(labels.contains(&"root"));
        assert!(labels.contains(&"dev"));
        assert!(labels.iter().any(|l| l.contains("/repo/wt-a")));
        assert!(labels.iter().any(|l| l.contains("/repo/wt-b")));
        // Path identity round-trips: each label maps to its own checkout.
        let path_of = |label: &str| {
            opts.iter()
                .find(|(l, _)| l == label)
                .map(|(_, p)| p.clone())
        };
        assert_eq!(
            path_of(labels.iter().find(|l| l.contains("wt-a")).unwrap()),
            Some(PathBuf::from("/repo/wt-a"))
        );
        assert_eq!(
            path_of(labels.iter().find(|l| l.contains("wt-b")).unwrap()),
            Some(PathBuf::from("/repo/wt-b"))
        );
    }

    #[test]
    fn root_worktree_never_duplicates() {
        // is_root flag AND a path-equal non-flagged entry are both skipped.
        let wts = wt_list(vec![
            worktree("main", "/repo", true),
            worktree("main", "/repo", false),
            worktree("feature", "/repo/wt", false),
        ]);
        let opts = checkout_options(Some("/repo"), &wts);
        assert_eq!(opts.len(), 2); // "root" + feature only
        assert!(opts.iter().any(|(l, _)| l == "root"));
        assert!(
            opts.iter()
                .any(|(l, p)| l == "feature" && p == &PathBuf::from("/repo/wt"))
        );
    }

    #[test]
    fn missing_worktrees_are_skipped() {
        let mut gone = worktree("gone", "/repo/gone", false);
        gone.missing = true;
        let wts = wt_list(vec![gone, worktree("ok", "/repo/ok", false)]);
        let opts = checkout_options(Some("/repo"), &wts);
        assert_eq!(opts.len(), 2);
        assert!(opts.iter().all(|(l, _)| l != "gone"));
    }

    #[test]
    fn the_notice_bar_dismiss_button_clears_the_notice() {
        use rune::{Env, Runtime};
        let mut s = State::new(crate::test_handle());
        s.view = crate::View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = std::path::PathBuf::from("/tmp/goko-notice-qa");
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.notice = Some(crate::editor::state::Notice {
            text: "stale data".into(),
            stale: true,
        });
        let mut rt = Runtime::new(s, crate::view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let dismiss = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "button" && e.label.as_deref() == Some("Dismiss notice"))
            .expect("the notice bar has a dismiss button");
        let (x, y) = (
            dismiss.rect[0] + dismiss.rect[2] / 2.0,
            dismiss.rect[1] + dismiss.rect[3] / 2.0,
        );
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.update(|s, _| assert!(s.editor.notice.is_none(), "the bar is dismissed"));
    }

    // -- Task 9: the empty Editor lists projects -----------------------------

    fn empty_editor(projects: Vec<ProjectView>) -> State {
        let mut s = State::new(crate::test_handle());
        s.view = crate::View::Editor;
        s.editor.projects = projects;
        s
    }

    fn project_rows(rt: &rune::Runtime<State>) -> Vec<rune::ElementInfo> {
        rt.elements()
            .into_iter()
            .filter(|e| {
                e.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("editor-empty-project:"))
            })
            .collect()
    }

    fn has_add_project(rt: &rune::Runtime<State>) -> bool {
        rt.elements()
            .iter()
            .any(|e| e.role == "button" && e.label.as_deref() == Some("Add project"))
    }

    #[test]
    fn three_projects_list_three_rows_beside_add_project() {
        let s = empty_editor(vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
            project("p3", "docs", "/work/c/docs"),
        ]);
        let mut rt = rune::Runtime::new(s, crate::view, (1100.0, 700.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);

        let rows = project_rows(&rt);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|e| e.role == "button"));
        assert_eq!(rows[0].label.as_deref(), Some("Open web"));
        assert!(has_add_project(&rt));
    }

    #[test]
    fn seven_projects_list_five_rows() {
        let projects = (1..=7)
            .map(|i| project(&format!("p{i}"), &format!("repo{i}"), &format!("/work/{i}")))
            .collect();
        let mut rt = rune::Runtime::new(
            empty_editor(projects),
            crate::view,
            (1100.0, 700.0),
            rune::Env::new(true, 2.0),
        );
        rt.frame(0.0);

        assert_eq!(project_rows(&rt).len(), 5);
        assert!(has_add_project(&rt));
    }

    #[test]
    fn pressing_a_row_selects_that_projects_root_checkout() {
        let s = empty_editor(vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ]);
        let mut rt = rune::Runtime::new(s, crate::view, (1100.0, 700.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);
        let row = project_rows(&rt)
            .into_iter()
            .find(|e| e.key.as_deref() == Some("editor-empty-project:p2"))
            .expect("the api row");
        let (x, y) = (
            row.rect[0] + row.rect[2] / 2.0,
            row.rect[1] + row.rect[3] / 2.0,
        );
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/b/api")));
    }

    #[test]
    fn zero_projects_keep_today_text_and_add_project() {
        let mut rt = rune::Runtime::new(
            empty_editor(Vec::new()),
            crate::view,
            (1100.0, 700.0),
            rune::Env::new(true, 2.0),
        );
        rt.frame(0.0);

        assert!(project_rows(&rt).is_empty());
        assert!(has_add_project(&rt));
        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(
            texts
                .iter()
                .any(|t| t == "No projects registered. Add a project folder to start editing.")
        );
    }

    #[test]
    fn loading_projects_says_so() {
        let mut s = empty_editor(Vec::new());
        s.editor.projects_loading = true;
        let mut rt = rune::Runtime::new(s, crate::view, (1100.0, 700.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);

        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(texts.iter().any(|t| t == "Loading projects…"));
        assert!(project_rows(&rt).is_empty());
    }
}

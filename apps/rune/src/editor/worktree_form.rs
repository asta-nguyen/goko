//! The native "New worktree" dialog — a port of the desktop
//! `features/worktrees/WorktreeCreateDialog.tsx`. All form state lives on
//! `EditorState` (`worktree_form`); every filesystem/Git call (branch and
//! base refs, env-file candidates, worktree creation, env copy) rides the
//! editor backend's worker lane, never the UI thread.
//!
//! While the dialog is open `s.sidebar_input` stays `Some(NewWorktree)`:
//! `main.rs::on_key` returns early for any sidebar input, so the modal owns
//! the keyboard without a main.rs change. `input_layer` renders nothing for
//! that variant — the prompt route is retired; the variant is the shield.

use std::path::PathBuf;

use app_core::worktrees::{
    EnvCopyReport, EnvFileCandidate, WorktreeBranches, WorktreeInfo, WorktreeList,
};
use rune::edit::TextEdit;
use rune::{El, Tx, col, el, row};

use crate::State;
use crate::editor::backend::{Job, Req};
use crate::editor::state::EditorState;
use crate::sidebar::SidebarInput;
use crate::theme::{self, Ink, T11, T11M, T12, T12M, T14M};

/// The `selectedCheckout` of the TSX: the checkout whose branch/commit a
/// new branch should start from, resolved at open.
#[derive(Clone, Debug)]
pub struct SeedCheckout {
    /// Branch of the checkout, or `None` when detached.
    pub branch: Option<String>,
    /// Short head, used as the base value and detached label.
    pub head: String,
}

/// Branch mode: create a new branch or check out an existing local one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    New,
    Existing,
}

/// Which `Select` listbox is open (at most one, like the other pickers).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pick {
    Base,
    Existing,
    Source,
}

/// Everything the dialog needs, kept on `EditorState` so replies and
/// handlers mutate one place. `None` = closed.
pub struct Form {
    /// Project the new worktree belongs to.
    pub project_id: String,
    /// Display name — used for the seeded path and the description line.
    pub project_name: String,
    /// The row's checkout (branch/head seeds `base`).
    pub seed: Option<SeedCheckout>,
    /// Checkout name; also the target directory's basename, kept in sync.
    pub name: TextEdit,
    /// Target directory for `git worktree add` (relative to the root or
    /// absolute — `worktrees::create` resolves it the same way the desktop
    /// path field does).
    pub path: TextEdit,
    pub mode: Mode,
    /// New-branch name (mode `New`).
    pub branch: TextEdit,
    /// `true` once the user edits `branch` directly — name edits stop
    /// syncing it (the TSX `branchTouched`).
    pub branch_touched: bool,
    /// Base ref or commit for a new branch.
    pub base: String,
    /// Selected local branch (mode `Existing`).
    pub existing: String,
    /// The open `Select`, if any.
    pub pick: Option<Pick>,
    /// Copy `.env*` files from another checkout (secrets stay local).
    pub copy_env: bool,
    /// Checkout path the env files copy from.
    pub env_source: String,
    /// Worker-loaded branches/bases for this project.
    pub branches: Option<WorktreeBranches>,
    pub branches_loading: bool,
    pub branches_error: Option<String>,
    /// Worker-loaded env-file preview for `env_requested`.
    pub env_files: Option<Vec<EnvFileCandidate>>,
    pub env_loading: bool,
    pub env_error: Option<String>,
    /// Source path the preview answers; a stale reply drops.
    pub env_requested: Option<String>,
    /// `Req::WorktreeCreate` on the lane — disables the form and blocks a
    /// second submit.
    pub submitting: bool,
    /// Inline error (validation or create failure) — the TSX `formError`.
    pub error: Option<String>,
}

/// The env-copy report the sidebar shows under the project once the form
/// closed — the desktop's `envResult` Alert, including the partial-success
/// case (worktree created, copy failed) and the per-file outcome list.
#[derive(Clone, Debug)]
pub struct EnvNotice {
    pub source_path: String,
    pub target_path: String,
    pub report: Option<EnvCopyReport>,
    /// The copy request itself failed (vs. per-file failures in `report`).
    pub error: Option<String>,
    /// A `Req::EnvCopy` retry is on the lane.
    pub retrying: bool,
}

// ---------------------------------------------------------------------------
// Path helpers — the TSX's dirname/basename/withName.
// ---------------------------------------------------------------------------

fn dirname(path: &str) -> &str {
    let value = path.trim_end_matches('/');
    match value.rfind('/') {
        Some(i) => &value[..i],
        None => "",
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Replace the path's basename with `name`, keeping its parent.
fn with_name(path: &str, name: &str) -> String {
    let parent = dirname(path);
    if !parent.is_empty() {
        format!("{parent}/{name}")
    } else if path.starts_with('/') {
        format!("/{name}")
    } else {
        name.to_string()
    }
}

/// The project's checkout list (validation + env sources): the sidebar's
/// per-project entry is the same data the picker uses.
fn worktrees_of<'a>(e: &'a EditorState, project_id: &str) -> Option<&'a WorktreeList> {
    e.sidebar_worktrees
        .get(project_id)
        .and_then(|entry| entry.list.as_ref())
}

fn free_branch_names(f: &Form) -> Vec<&str> {
    f.branches
        .iter()
        .flat_map(|refs| refs.branches.iter())
        .filter(|b| b.checked_out_at.is_none())
        .map(|b| b.name.as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// Open / close.
// ---------------------------------------------------------------------------

/// Open the form for `project_id`. `checkout_path` is the row's checkout
/// (project root or a worktree path) — it seeds `base` like the desktop's
/// `selectedCheckout` prop.
pub fn open(s: &mut State, project_id: &str, checkout_path: &str) {
    let Some(project) = s
        .editor
        .projects
        .iter()
        .find(|p| p.project.id == project_id)
        .map(|p| p.project.clone())
    else {
        s.notice = Some("the project is no longer registered".into());
        return;
    };
    crate::editor::keyboard::clear_edit_focuses(s);
    // The checkout the row points at seeds the base; a detached checkout
    // contributes its head commit instead of a branch name.
    let seed = worktrees_of(&s.editor, project_id)
        .and_then(|list| {
            list.worktrees
                .iter()
                .find(|w| w.path == checkout_path)
                // A stored root may differ from the canonical reported path;
                // the root worktree is the sensible fallback seed.
                .or_else(|| list.worktrees.iter().find(|w| w.is_root))
        })
        .map(|w| SeedCheckout {
            branch: w.branch.clone(),
            head: w.head.clone(),
        });
    let base = seed
        .as_ref()
        .map(|seed| seed.branch.clone().unwrap_or_else(|| seed.head.clone()))
        .unwrap_or_default();
    let mut name = TextEdit::new("worktree");
    name.focused = true;
    s.editor.worktree_form = Some(Form {
        project_id: project_id.to_string(),
        project_name: project.name.clone(),
        seed,
        name,
        path: TextEdit::new(format!("../{}-worktrees/worktree", project.name)),
        mode: Mode::New,
        branch: TextEdit::new("worktree"),
        branch_touched: false,
        base,
        existing: String::new(),
        pick: None,
        copy_env: false,
        env_source: project.root.clone(),
        branches: None,
        branches_loading: false,
        branches_error: None,
        env_files: None,
        env_loading: false,
        env_error: None,
        env_requested: None,
        submitting: false,
        error: None,
    });
    // The keyboard shield: `main.rs::on_key` returns early while any
    // sidebar input is open, so the dialog owns the keyboard. Cleared by
    // `close` (UI paths) and `after_reply` (the success reply, which runs
    // at EditorState level and cannot reach `s.sidebar_input`).
    s.sidebar_input = Some(SidebarInput::NewWorktree {
        project_id: project_id.to_string(),
    });
    // The form needs the worktree list (name collisions, env sources) —
    // `request_worktrees` dedupes with an in-flight read and warms a cold
    // entry — plus the branch/base refs.
    s.editor.request_worktrees(project_id);
    let sent = s.editor.backend.send(Job::project(Req::WorktreeBranches {
        project_id: project_id.to_string(),
    }));
    if let Some(f) = s.editor.worktree_form.as_mut() {
        f.branches_loading = sent;
        if !sent {
            f.branches_error = Some("the editor queue is full; branches were not requested".into());
        }
    }
}

/// Dismiss requested by the UI (Cancel, Escape, outside press): blocked
/// while the create job is in flight, like the desktop's `disabled` guard.
pub fn request_close(s: &mut State) {
    if s.editor
        .worktree_form
        .as_ref()
        .is_some_and(|f| f.submitting)
    {
        return;
    }
    close(s);
}

/// Close the dialog and release the keyboard shield.
pub fn close(s: &mut State) {
    s.editor.worktree_form = None;
    if matches!(s.sidebar_input, Some(SidebarInput::NewWorktree { .. })) {
        s.sidebar_input = None;
    }
}

/// State-level post-reply housekeeping, called from the backend worker's
/// `handle.update` closure right after `apply_reply`: a successful create
/// clears `worktree_form` at EditorState level, which cannot reach
/// `s.sidebar_input` — release the shield here so the app never stays
/// keyboard-blind.
pub(crate) fn after_reply(s: &mut State) {
    if s.editor.worktree_form.is_none()
        && matches!(s.sidebar_input, Some(SidebarInput::NewWorktree { .. }))
    {
        s.sidebar_input = None;
    }
}

/// Fresh branch/base data for the open form: default `existing` to the
/// first free branch and keep it valid as the list changes.
pub fn branches_loaded(f: &mut Form) {
    let free = free_branch_names(f);
    if f.existing.is_empty() || !free.contains(&f.existing.as_str()) {
        f.existing = free.first().unwrap_or(&"").to_string();
    }
}

// ---------------------------------------------------------------------------
// Submit — the TSX's `submit` validation, verbatim messages.
// ---------------------------------------------------------------------------

/// The validation half of `submit` as a pure check: `Some` = the formError
/// text; `None` = the job may be issued. Split out for tests.
fn validate(list: Option<&WorktreeList>, f: &Form) -> Option<String> {
    let name = f.name.text.as_str();
    if name.trim().is_empty()
        || name != name.trim()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
    {
        return Some("Name must be one directory name.".into());
    }
    if list.is_some_and(|list| list.worktrees.iter().any(|w| w.name == name)) {
        return Some(format!("A checkout named “{name}” already exists."));
    }
    if f.path.text.trim().is_empty() {
        return Some("Choose a path for the worktree.".into());
    }
    // A checkout that went missing after selection is a known-invalid copy
    // source — block here rather than manufacturing a partial success.
    if f.copy_env
        && list.is_some_and(|list| {
            list.worktrees
                .iter()
                .any(|w| w.path == f.env_source && w.missing)
        })
    {
        return Some(format!(
            "The copy source {} is missing. Choose another checkout.",
            f.env_source
        ));
    }
    match f.mode {
        Mode::New => {
            if f.branch.text.trim().is_empty() {
                return Some("Enter a branch name.".into());
            }
            let Some(refs) = f.branches.as_ref().filter(|_| f.branches_error.is_none()) else {
                return Some(
                    "Could not load local branches. Retry before creating a worktree.".into(),
                );
            };
            if refs.branches.iter().any(|b| b.name == f.branch.text.trim()) {
                return Some(format!(
                    "Branch “{}” already exists. Choose Existing branch or use another name.",
                    f.branch.text.trim()
                ));
            }
        }
        Mode::Existing => {
            let Some(refs) = f.branches.as_ref().filter(|_| f.branches_error.is_none()) else {
                return Some(
                    "Could not load local branches. Retry before creating a worktree.".into(),
                );
            };
            match refs.branches.iter().find(|b| b.name == f.existing) {
                Some(item) if item.checked_out_at.is_none() => {}
                Some(item) => {
                    return Some(format!(
                        "Branch “{}” is checked out at {}.",
                        f.existing,
                        item.checked_out_at.as_deref().unwrap_or_default()
                    ));
                }
                None => return Some("Choose an existing local branch.".into()),
            }
        }
    }
    None
}

/// Validate, then queue `Req::WorktreeCreate` on the worker lane. A refused
/// queue unwinds `submitting` so the user can retry.
pub fn submit(s: &mut State) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    if f.submitting {
        return;
    }
    let project_id = f.project_id.clone();
    let list = s
        .editor
        .sidebar_worktrees
        .get(&project_id)
        .and_then(|entry| entry.list.as_ref());
    if let Some(error) = validate(list, f) {
        f.error = Some(error);
        return;
    }
    f.error = None;
    f.submitting = true;
    let f = s.editor.worktree_form.as_mut().unwrap();
    let (mode, branch, base) = match f.mode {
        Mode::New => (
            "new",
            f.branch.text.trim().to_string(),
            Some(f.base.clone()),
        ),
        Mode::Existing => ("existing", f.existing.clone(), None),
    };
    let sent = s.editor.backend.send(Job::project(Req::WorktreeCreate {
        project_id,
        path: f.path.text.trim().to_string(),
        mode: mode.to_string(),
        branch,
        base: base.filter(|b| !b.is_empty()),
        env_source: if f.copy_env {
            Some(f.env_source.clone())
        } else {
            None
        },
    }));
    if let Some(f) = s.editor.worktree_form.as_mut()
        && !sent
    {
        f.submitting = false;
        f.error = Some("the editor queue is full; the worktree was not created".into());
    }
}

/// Queue `Req::EnvCandidates` for the preview when `copy_env` is on and the
/// source changed. Dedupes on `env_requested` so a reply can't loop.
fn request_env_preview(s: &mut State) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    if !f.copy_env || f.env_requested.as_deref() == Some(f.env_source.as_str()) {
        return;
    }
    let (project_id, source) = (f.project_id.clone(), f.env_source.clone());
    f.env_requested = Some(source.clone());
    f.env_loading = true;
    f.env_error = None;
    f.env_files = None;
    let sent = s.editor.backend.send(Job::project(Req::EnvCandidates {
        project_id,
        source_path: source,
    }));
    if let Some(f) = s.editor.worktree_form.as_mut()
        && !sent
    {
        f.env_loading = false;
        // The refusal leaves no request in flight: release the latch so a
        // deliberate retry (checkbox toggle, Retry) can re-issue.
        f.env_requested = None;
        f.env_error = Some("the editor queue is full; files were not scanned".into());
    }
}

/// The sidebar notice's "Retry copy": re-issue the copy for the recorded
/// source/target pair.
pub fn retry_env_copy(s: &mut State, project_id: &str) {
    let Some(notice) = s.editor.worktree_env.get_mut(project_id) else {
        return;
    };
    if notice.retrying {
        return;
    }
    notice.retrying = true;
    let (source_path, target_path) = (notice.source_path.clone(), notice.target_path.clone());
    let sent = s.editor.backend.send(Job::project(Req::EnvCopy {
        project_id: project_id.to_string(),
        source_path,
        target_path,
    }));
    if !sent && let Some(notice) = s.editor.worktree_env.get_mut(project_id) {
        notice.retrying = false;
        notice.error = Some("the editor queue is full; copy was not retried".into());
    }
}

// ---------------------------------------------------------------------------
// Field interlocks — the TSX's name/path/branch sync.
// ---------------------------------------------------------------------------

fn name_changed(s: &mut State) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    let name = f.name.text.clone();
    let path = with_name(&f.path.text, &name);
    f.path.replace_text(path);
    if f.mode == Mode::New && !f.branch_touched {
        f.branch.replace_text(&name);
    }
}

fn path_changed(s: &mut State) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    let name = basename(&f.path.text).to_string();
    if name.is_empty() {
        return;
    }
    f.name.replace_text(&name);
    if f.mode == Mode::New && !f.branch_touched {
        f.branch.replace_text(name);
    }
}

fn set_mode(s: &mut State, mode: Mode) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    if f.mode == mode {
        return;
    }
    f.mode = mode;
    f.pick = None;
    match mode {
        Mode::New => {
            let name = f.name.text.clone();
            f.branch.replace_text(name);
            f.branch_touched = false;
        }
        Mode::Existing => {
            let free = free_branch_names(f);
            f.existing = free.first().unwrap_or(&"").to_string();
        }
    }
}

fn toggle_pick(s: &mut State, pick: Pick) {
    let Some(f) = s.editor.worktree_form.as_mut() else {
        return;
    };
    f.pick = if f.pick == Some(pick) {
        None
    } else {
        Some(pick)
    };
}

fn close_pick(s: &mut State) {
    if let Some(f) = s.editor.worktree_form.as_mut() {
        f.pick = None;
    }
}

/// The native folder picker fills the path, and the name/branch interlock
/// follows like a typed path edit (the TSX `choosePath`).
fn choose_path(s: &mut State, tx: &mut Tx) {
    let Some(f) = s.editor.worktree_form.as_ref() else {
        return;
    };
    if f.submitting {
        return;
    }
    let directory = s
        .editor
        .projects
        .iter()
        .find(|p| p.project.id == f.project_id)
        .map(|p| PathBuf::from(&p.project.root));
    tx.open_folder(
        rune::file_dialog::FileDialogOptions {
            title: "Choose the worktree folder".into(),
            directory,
            ..Default::default()
        },
        |st: &mut State, result: rune::file_dialog::FileDialogResult, _tx: &mut Tx| {
            let path = match result {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(e) => {
                    st.notice = Some(e.to_string());
                    return;
                }
            };
            let Some(f) = st.editor.worktree_form.as_mut() else {
                return;
            };
            let text = path.display().to_string();
            f.path.replace_text(&text);
            let name = basename(&text).to_string();
            if !name.is_empty() {
                f.name.replace_text(&name);
                if f.mode == Mode::New && !f.branch_touched {
                    f.branch.replace_text(name);
                }
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Option lists — `(label, value)` pairs; the Select matches on the label,
// so checked-out and missing rows carry a suffix instead of being dropped.
// ---------------------------------------------------------------------------

fn base_options(f: &Form) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(refs) = &f.branches {
        for item in &refs.bases {
            let label = if item.kind == "remote" {
                format!("{} (remote)", item.name)
            } else {
                item.name.clone()
            };
            pairs.push((label, item.name.clone()));
        }
    }
    // The synthetic current-base option: a detached head (or a base the ref
    // list doesn't carry) stays selectable, like the TSX's `hasCurrentBase`.
    if let Some(seed) = &f.seed {
        let value = seed.branch.clone().unwrap_or_else(|| seed.head.clone());
        if !value.is_empty() && !pairs.iter().any(|(_, v)| *v == value) {
            let label = seed
                .branch
                .clone()
                .unwrap_or_else(|| format!("detached {} (commit)", seed.head));
            pairs.insert(0, (label, value));
        }
    }
    pairs
}

fn existing_options(f: &Form) -> Vec<(String, String)> {
    f.branches
        .iter()
        .flat_map(|refs| refs.branches.iter())
        .map(|item| {
            let label = match &item.checked_out_at {
                Some(at) => format!("{} — checked out at {at}", item.name),
                None => item.name.clone(),
            };
            (label, item.name.clone())
        })
        .collect()
}

/// Copy sources: existing checkouts only — the desktop disables missing
/// rows, so they never enter the option list at all.
fn source_options(worktrees: &[WorktreeInfo]) -> Vec<(String, String)> {
    worktrees
        .iter()
        .filter(|w| !w.missing)
        .map(|w| {
            let mut label = w.name.clone();
            if w.is_root {
                label.push_str(" (project root)");
            }
            (label, w.path.clone())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The dialog layer.
// ---------------------------------------------------------------------------

/// The "New worktree" modal, matching `sidebar::input_layer`'s mount
/// contract: scrim+card return as one overlay `El` while `Select` popups
/// collect into `layers`. `None` while the form is closed.
pub fn layer(s: &State, layers: &mut Vec<El>) -> Option<El> {
    let f = s.editor.worktree_form.as_ref()?;
    let ink = &s.ink;
    let k = &s.editor.kit;
    let busy = f.submitting;

    // Text fields; `on_submit` is Enter → the same validation path as the
    // Create button.
    let name_input = rune_kit::TextInput::new(&f.name)
        .placeholder("worktree")
        .width(None)
        .on_edit(|st: &mut State| -> &mut TextEdit {
            &mut st.editor.worktree_form.as_mut().unwrap().name
        })
        .on_change(|st: &mut State, _tx: &mut Tx| name_changed(st))
        .on_submit(|st: &mut State, _tx: &mut Tx| submit(st))
        .build(k);
    let path_input = rune_kit::TextInput::new(&f.path)
        .placeholder("../<project>-worktrees/worktree")
        .width(None)
        .on_edit(|st: &mut State| -> &mut TextEdit {
            &mut st.editor.worktree_form.as_mut().unwrap().path
        })
        .on_change(|st: &mut State, _tx: &mut Tx| path_changed(st))
        .on_submit(|st: &mut State, _tx: &mut Tx| submit(st))
        .build(k);
    let branch_input = rune_kit::TextInput::new(&f.branch)
        .placeholder("worktree")
        .width(None)
        .on_edit(|st: &mut State| -> &mut TextEdit {
            &mut st.editor.worktree_form.as_mut().unwrap().branch
        })
        .on_change(|st: &mut State, _tx: &mut Tx| {
            if let Some(f) = st.editor.worktree_form.as_mut() {
                f.branch_touched = true;
            }
        })
        .on_submit(|st: &mut State, _tx: &mut Tx| submit(st))
        .build(k);

    let name_field = rune_kit::Field::new()
        .label("Name")
        .control(name_input.w_full())
        .build(k);
    let path_field = rune_kit::Field::new()
        .label("Path")
        .control(
            row()
                .w_full()
                .gap(8.0)
                .child(path_input.grow(1.0).min_w(0.0))
                .child(
                    theme::btn_sm("Choose…", None, true, ink)
                        .disabled(busy)
                        .on_press_tx(choose_path),
                ),
        )
        .build(k);

    // Branch fieldset: mode radios, then the mode's own fields. "Existing
    // local branch" disables when no free local branch exists — the TSX's
    // `disabled={freeBranches.length === 0}`.
    let free = free_branch_names(f);
    let radio = rune_kit::RadioGroup::new()
        .disabled(busy)
        .item(
            rune_kit::RadioItem::new("New branch", f.mode == Mode::New)
                .on_select(|st: &mut State| set_mode(st, Mode::New)),
        )
        .item(
            rune_kit::RadioItem::new("Existing local branch", f.mode == Mode::Existing)
                .disabled(free.is_empty() && f.mode != Mode::Existing)
                .on_select(|st: &mut State| set_mode(st, Mode::Existing)),
        )
        .build(k);

    let branch_fields = match f.mode {
        Mode::New => {
            let pairs = base_options(f);
            let labels: Vec<String> = pairs.iter().map(|(l, _)| l.clone()).collect();
            let current = pairs
                .iter()
                .find(|(_, v)| *v == f.base)
                .map(|(l, _)| l.clone());
            let choose = pairs.clone();
            let base_select = rune_kit::Select::new("worktree-base", &labels)
                .appearance(rune_kit::SelectAppearance::Field)
                .current(current.as_deref())
                .placeholder(if f.branches_loading {
                    "Loading…"
                } else {
                    "Start from"
                })
                .label("Start from")
                .width(240.0)
                .open(f.pick == Some(Pick::Base))
                .on_toggle(|st: &mut State| toggle_pick(st, Pick::Base))
                .on_close(close_pick)
                .on_choose(move |st: &mut State, label: &str, _tx: &mut Tx| {
                    if let Some((_, value)) = choose.iter().find(|(l, _)| l == label)
                        && let Some(f) = st.editor.worktree_form.as_mut()
                    {
                        f.base = value.clone();
                        f.pick = None;
                    }
                })
                .build(k, layers);
            col()
                .w_full()
                .gap(8.0)
                .child(
                    rune_kit::Field::new()
                        .label("Branch name")
                        .control(branch_input.w_full())
                        .build(k),
                )
                .child(
                    rune_kit::Field::new()
                        .label("Start from")
                        .control(base_select.w_full())
                        .build(k),
                )
        }
        Mode::Existing => {
            let pairs = existing_options(f);
            let labels: Vec<String> = pairs.iter().map(|(l, _)| l.clone()).collect();
            let current = pairs
                .iter()
                .find(|(_, v)| *v == f.existing)
                .map(|(l, _)| l.clone());
            let choose = pairs.clone();
            let branch_select = rune_kit::Select::new("worktree-existing", &labels)
                .appearance(rune_kit::SelectAppearance::Field)
                .current(current.as_deref())
                .placeholder(if f.branches_loading {
                    "Loading…"
                } else if labels.is_empty() {
                    "No local branches"
                } else {
                    "Local branch"
                })
                .label("Local branch")
                .width(300.0)
                .open(f.pick == Some(Pick::Existing))
                .on_toggle(|st: &mut State| toggle_pick(st, Pick::Existing))
                .on_close(close_pick)
                .on_choose(move |st: &mut State, label: &str, _tx: &mut Tx| {
                    if let Some((_, value)) = choose.iter().find(|(l, _)| l == label)
                        && let Some(f) = st.editor.worktree_form.as_mut()
                    {
                        f.existing = value.clone();
                        f.pick = None;
                    }
                })
                .build(k, layers);
            col().w_full().child(
                rune_kit::Field::new()
                    .label("Local branch")
                    .control(branch_select.w_full())
                    .build(k),
            )
        }
    };
    let branch_box = col()
        .w_full()
        .gap(10.0)
        .padding(10.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .child(theme::txt("Branch", T12M, ink.text))
        .child(radio)
        .child(branch_fields)
        .child(theme::txt(
            "Goko uses refs already available locally and does not fetch.",
            T11,
            ink.muted,
        ));

    // Environment files fieldset: the copy toggle, the source checkout and
    // the scanned preview (copy / skip reason per file).
    let checkbox = rune_kit::Checkbox::new(f.copy_env)
        .label("Copy environment files into this worktree")
        .description(
            "These files may contain secrets. They are copied locally and are not committed.",
        )
        .disabled(busy)
        .on_toggle(|st: &mut State| {
            if let Some(f) = st.editor.worktree_form.as_mut() {
                f.copy_env = !f.copy_env;
            }
            request_env_preview(st);
        })
        .build(k);
    let mut env_box = col()
        .w_full()
        .gap(10.0)
        .padding(10.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .child(theme::txt("Environment files", T12M, ink.text))
        .child(checkbox);
    if f.copy_env {
        let worktrees = worktrees_of(&s.editor, &f.project_id)
            .map(|l| l.worktrees.as_slice())
            .unwrap_or(&[]);
        let pairs = source_options(worktrees);
        let labels: Vec<String> = pairs.iter().map(|(l, _)| l.clone()).collect();
        let current = pairs
            .iter()
            .find(|(_, v)| *v == f.env_source)
            .map(|(l, _)| l.clone());
        let choose = pairs.clone();
        let source_select = rune_kit::Select::new("worktree-env-source", &labels)
            .appearance(rune_kit::SelectAppearance::Field)
            .current(current.as_deref())
            .placeholder("Copy from")
            .label("Copy from")
            .width(300.0)
            .open(f.pick == Some(Pick::Source))
            .on_toggle(|st: &mut State| toggle_pick(st, Pick::Source))
            .on_close(close_pick)
            .on_choose(move |st: &mut State, label: &str, _tx: &mut Tx| {
                let mut changed = false;
                if let Some((_, value)) = choose.iter().find(|(l, _)| l == label)
                    && let Some(f) = st.editor.worktree_form.as_mut()
                {
                    f.env_source = value.clone();
                    f.pick = None;
                    changed = true;
                }
                if changed {
                    request_env_preview(st);
                }
            })
            .build(k, layers);
        env_box = env_box.child(
            rune_kit::Field::new()
                .label("Copy from")
                .control(source_select.w_full())
                .build(k),
        );
        // Copy preview — scanned worker-side so the UI never reads `.env*`.
        let mut preview = col()
            .w_full()
            .gap(4.0)
            .padding(8.0)
            .radius(6.0)
            .fill(ink.bg.alpha(0.7))
            .child(theme::txt("Copy preview", T11M, ink.text));
        if f.env_loading {
            preview = preview.child(theme::txt("Scanning files…", T11, ink.muted));
        } else if let Some(error) = &f.env_error {
            preview = preview.child(
                row()
                    .w_full()
                    .gap(8.0)
                    .items_center()
                    .child(theme::txt(error, T11, ink.danger).grow(1.0))
                    .child(
                        theme::btn_sm("Retry", None, true, ink)
                            .disabled(busy)
                            .on_press(|st: &mut State| {
                                if let Some(f) = st.editor.worktree_form.as_mut() {
                                    f.env_requested = None;
                                }
                                request_env_preview(st);
                            }),
                    ),
            );
        } else if let Some(files) = &f.env_files {
            if files.is_empty() {
                preview = preview.child(theme::txt("No environment files found.", T11, ink.muted));
            } else {
                for item in files {
                    let (mark, tone) = if item.eligible {
                        ("copy", ink.success)
                    } else {
                        (item.reason.as_deref().unwrap_or("skip"), ink.muted)
                    };
                    preview = preview.child(
                        row()
                            .w_full()
                            .gap(12.0)
                            .child(theme::txt(&item.name, T11, ink.text).grow(1.0))
                            .child(theme::txt(mark, T11, tone)),
                    );
                }
            }
        }
        env_box = env_box.child(preview);
    }

    // Validation / create error, then the footer buttons — `disabled`
    // blocks a second submit while the job runs (the TSX's `isPending`).
    let mut card = col()
        .key("worktree-form")
        .role("dialog")
        .label("New worktree")
        .description(format!(
            "Create an independent checkout inside {}. It stays part of this project.",
            f.project_name
        ))
        .w(560.0)
        .gap(14.0)
        .padding(16.0)
        .radius(10.0)
        .fill(ink.panel)
        .border(1.0, ink.border)
        .child(theme::txt("New worktree", T14M, ink.text))
        .child(theme::txt(
            &format!(
                "Create an independent checkout inside {}. It stays part of this project.",
                f.project_name
            ),
            T12,
            ink.muted,
        ))
        .child(col().w_full().gap(10.0).child(name_field).child(path_field))
        .child(branch_box)
        .child(env_box);
    if let Some(error) = &f.error {
        card = card.child(el().role("alert").child(theme::txt(error, T12, ink.danger)));
    }
    if let Some(error) = &f.branches_error {
        card = card.child(el().role("alert").child(theme::txt(error, T12, ink.danger)));
    }
    card = card.child(
        row()
            .w_full()
            .justify_end()
            .gap(8.0)
            .child(
                theme::btn_sm("Cancel", None, true, ink)
                    .disabled(busy)
                    .on_press(|st: &mut State| request_close(st)),
            )
            .child(
                theme::btn_sm(
                    if busy {
                        "Creating…"
                    } else {
                        "Create worktree"
                    },
                    None,
                    false,
                    ink,
                )
                .disabled(busy || f.branches_loading || f.path.text.trim().is_empty())
                .role("button")
                .label("Create worktree")
                .on_press(|st: &mut State| submit(st)),
            ),
    );
    // Focus trap + background inert + Escape → request_close (`El::modal`);
    // outside presses dismiss too — `request_close` refuses while busy.
    let card = card.modal(!busy, request_close).modal_click_outside(!busy);

    let scrim = el()
        .key("worktree-form-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(ink.bg.alpha(0.55))
        .a11y_hidden();
    Some(
        el().key("worktree-form-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .child(scrim)
            .child(
                col()
                    .key("worktree-form-center")
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .center()
                    .child(card),
            ),
    )
}

// ---------------------------------------------------------------------------
// The sidebar env-copy notice — the desktop's `envResult` Alert, shown
// under the project's worktree group once creation + copy resolved.
// ---------------------------------------------------------------------------

/// Render one project's env-copy report card (title + per-file outcomes +
/// Retry/Dismiss). Called by `sidebar::worktree_group`.
pub fn env_notice_card(_s: &State, ink: &Ink, project_id: &str, notice: &EnvNotice) -> El {
    let title = if let Some(error) = &notice.error {
        format!(
            "Worktree created, but environment files could not be copied from {}: {}",
            notice.source_path, error
        )
    } else if notice.report.as_ref().is_some_and(|r| !r.failed.is_empty()) {
        format!(
            "Worktree created; some environment files could not be copied from {}.",
            notice.source_path
        )
    } else {
        format!("Environment copy from {} completed.", notice.source_path)
    };
    let danger_tone =
        notice.error.is_some() || notice.report.as_ref().is_some_and(|r| !r.failed.is_empty());
    let mut card = col()
        .key(format!("worktree-env:{project_id}"))
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(6.0)
        .fill(ink.text.alpha(0.05))
        .child(theme::txt(
            &title,
            T11,
            if danger_tone { ink.danger } else { ink.muted },
        ));
    if let Some(report) = &notice.report {
        let mut rows = col().w_full().gap(2.0).pl(10.0);
        for name in &report.copied {
            rows = rows.child(theme::txt(&format!("{name}: copied"), T11, ink.muted));
        }
        for name in &report.skipped {
            rows = rows.child(theme::txt(
                &format!("{name}: skipped (already exists or excluded)"),
                T11,
                ink.muted,
            ));
        }
        for item in &report.failed {
            rows = rows.child(theme::txt(
                &format!("{}: {}", item.name, item.error),
                T11,
                ink.danger,
            ));
        }
        if report.copied.is_empty() && report.skipped.is_empty() && report.failed.is_empty() {
            rows = rows.child(theme::txt("No environment files found.", T11, ink.muted));
        }
        card = card.child(rows);
    }
    let can_retry =
        notice.error.is_some() || notice.report.as_ref().is_some_and(|r| !r.failed.is_empty());
    let pid = project_id.to_string();
    let mut actions = row().gap(6.0).justify_end();
    if can_retry {
        let retry_pid = pid.clone();
        actions = actions.child(
            theme::btn_sm(
                if notice.retrying {
                    "Retrying…"
                } else {
                    "Retry copy"
                },
                None,
                true,
                ink,
            )
            .disabled(notice.retrying)
            .on_press(move |st: &mut State| retry_env_copy(st, &retry_pid)),
        );
    }
    actions = actions.child(theme::btn_sm("Dismiss", None, true, ink).on_press(
        move |st: &mut State| {
            st.editor.worktree_env.remove(&pid);
        },
    ));
    card.child(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::projects::add_project;
    use app_core::store::Storage;
    use app_core::test_support::TempDir;
    use app_core::worktrees::BranchInfo;
    use std::path::Path;
    use std::process::Command;

    fn git_ok(root: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo(dir: &TempDir) -> PathBuf {
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Goko Test"]);
        git_ok(&root, &["config", "user.email", "goko@example.invalid"]);
        std::fs::write(root.join("tracked.txt"), "initial\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "-m", "initial"]);
        root
    }

    fn branch(name: &str, checked_out_at: Option<&str>) -> BranchInfo {
        BranchInfo {
            name: name.into(),
            checked_out_at: checked_out_at.map(str::to_string),
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

    fn form() -> Form {
        Form {
            project_id: "p1".into(),
            project_name: "web".into(),
            seed: None,
            name: TextEdit::new("worktree"),
            path: TextEdit::new("../web-worktrees/worktree"),
            mode: Mode::New,
            branch: TextEdit::new("worktree"),
            branch_touched: false,
            base: "main".into(),
            existing: String::new(),
            pick: None,
            copy_env: false,
            env_source: "/root".into(),
            branches: None,
            branches_loading: false,
            branches_error: None,
            env_files: None,
            env_loading: false,
            env_error: None,
            env_requested: None,
            submitting: false,
            error: None,
        }
    }

    // -- validation (the TSX formError rules, verbatim) ----------------------

    #[test]
    fn name_must_be_one_directory_name() {
        for bad in ["", " ", " name", "name ", ".", "..", "a/b", "a\\b"] {
            let mut f = form();
            f.name = TextEdit::new(bad);
            assert_eq!(
                validate(None, &f).as_deref(),
                Some("Name must be one directory name."),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn name_collision_against_checkout_list() {
        let mut f = form();
        f.name = TextEdit::new("feat");
        let list = WorktreeList {
            repo: true,
            worktrees: vec![
                wt("web", "/work/web", true),
                wt("feat", "/work/feat", false),
            ],
        };
        assert_eq!(
            validate(Some(&list), &f).as_deref(),
            Some("A checkout named “feat” already exists.")
        );
    }

    #[test]
    fn empty_path_and_missing_branch_data() {
        let mut f = form();
        f.path = TextEdit::new("  ");
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Choose a path for the worktree.")
        );
        // Branches not yet loaded: refuse instead of guessing.
        f.path = TextEdit::new("/tmp/x");
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Could not load local branches. Retry before creating a worktree.")
        );
        f.branches_error = Some("git exploded".into());
        f.branches = Some(WorktreeBranches {
            branches: Vec::new(),
            bases: Vec::new(),
        });
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Could not load local branches. Retry before creating a worktree."),
            "an error reply also blocks create"
        );
    }

    #[test]
    fn new_branch_rejects_existing_and_empty_names() {
        let mut f = form();
        f.branch = TextEdit::new("  ");
        f.branches = Some(WorktreeBranches {
            branches: vec![branch("main", None)],
            bases: Vec::new(),
        });
        assert_eq!(validate(None, &f).as_deref(), Some("Enter a branch name."));
        f.branch = TextEdit::new("main");
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Branch “main” already exists. Choose Existing branch or use another name.")
        );
        f.branch = TextEdit::new("feat/x");
        assert_eq!(validate(None, &f), None);
    }

    #[test]
    fn existing_mode_requires_a_free_local_branch() {
        let mut f = form();
        f.mode = Mode::Existing;
        f.existing = "main".into();
        f.branches = Some(WorktreeBranches {
            branches: vec![branch("main", Some("/work/web")), branch("feat", None)],
            bases: Vec::new(),
        });
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Branch “main” is checked out at /work/web.")
        );
        f.existing = "feat".into();
        assert_eq!(validate(None, &f), None);
        f.existing = "ghost".into();
        assert_eq!(
            validate(None, &f).as_deref(),
            Some("Choose an existing local branch.")
        );
    }

    #[test]
    fn branches_loaded_seeds_first_free_branch() {
        let mut f = form();
        f.branches = Some(WorktreeBranches {
            branches: vec![branch("main", Some("/work/web")), branch("feat", None)],
            bases: Vec::new(),
        });
        branches_loaded(&mut f);
        assert_eq!(f.existing, "feat");
        // A stale selection snaps back to the first free branch.
        f.existing = "ghost".into();
        branches_loaded(&mut f);
        assert_eq!(f.existing, "feat");
    }

    #[test]
    fn path_helpers_match_the_tsx() {
        assert_eq!(with_name("../a/worktree", "x"), "../a/x");
        // `/abs`'s dirname is "" — the TSX swaps in `/{name}` here too.
        assert_eq!(with_name("/abs", "x"), "/x");
        assert_eq!(with_name("bare", "x"), "x");
        assert_eq!(basename("../a/wt/"), "wt");
        assert_eq!(dirname("../a/wt"), "../a");
    }

    // -- the worker job, end to end against a real repo ----------------------

    fn storage_and_project(dir: &TempDir, root: &Path) -> (Option<Storage>, String) {
        let storage = Storage::open(dir.path());
        let store = storage.get().unwrap();
        let pv = add_project(store, root.to_str().unwrap()).unwrap();
        (Some(storage), pv.project.id)
    }

    #[test]
    fn worktree_create_job_makes_the_checkout_and_reports_env_copy() {
        let dir = TempDir::new("wt-form-create");
        let root = repo(&dir);
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(root.join(".env.example"), "SECRET=\n").unwrap();
        let (storage, pid) = storage_and_project(&dir, &root);
        let target = dir.path().join("wt-out");

        // With env copy: `.env` lands, `.env.example` is skipped by rule.
        let reply = crate::editor::backend::run_job(
            &storage,
            &Job::project(Req::WorktreeCreate {
                project_id: pid.clone(),
                path: target.to_str().unwrap().to_string(),
                mode: "new".into(),
                branch: "feat/worktree".into(),
                base: Some("main".into()),
                env_source: Some(root.to_str().unwrap().to_string()),
            }),
        );
        let crate::editor::backend::ReplyData::WorktreeCreated(created) = reply.result.unwrap()
        else {
            panic!("expected WorktreeCreated")
        };
        assert_eq!(created.info.branch.as_deref(), Some("feat/worktree"));
        let env = created.env.expect("env copy requested");
        let report = env.expect("copy succeeds");
        assert_eq!(report.copied, vec![".env".to_string()]);
        assert!(
            Path::new(&created.info.path).join(".env").is_file(),
            "the secret file arrived in the new checkout"
        );
        assert!(!Path::new(&created.info.path).join(".env.example").exists());
    }

    #[test]
    fn worktree_create_reports_partial_success_when_copy_fails() {
        let dir = TempDir::new("wt-form-partial");
        let root = repo(&dir);
        let (storage, pid) = storage_and_project(&dir, &root);
        let target = dir.path().join("wt-partial");

        // A source that is not a registered checkout makes env_copy fail —
        // the worktree still exists: partial success, not a job error.
        let reply = crate::editor::backend::run_job(
            &storage,
            &Job::project(Req::WorktreeCreate {
                project_id: pid,
                path: target.to_str().unwrap().to_string(),
                mode: "new".into(),
                branch: "feat/partial".into(),
                base: Some("main".into()),
                env_source: Some("/definitely/not/a/checkout".into()),
            }),
        );
        let crate::editor::backend::ReplyData::WorktreeCreated(created) = reply.result.unwrap()
        else {
            panic!("expected WorktreeCreated")
        };
        assert!(
            Path::new(&created.info.path).is_dir(),
            "the worktree was created despite the copy failure"
        );
        assert_eq!(created.info.branch.as_deref(), Some("feat/partial"));
        let error = created
            .env
            .expect("env copy requested")
            .expect_err("copy failed");
        assert!(!error.is_empty());
    }

    #[test]
    fn worktree_create_rejects_a_checked_out_branch() {
        let dir = TempDir::new("wt-form-checked");
        let root = repo(&dir);
        let (storage, pid) = storage_and_project(&dir, &root);
        let target = dir.path().join("wt-blocked");
        let reply = crate::editor::backend::run_job(
            &storage,
            &Job::project(Req::WorktreeCreate {
                project_id: pid,
                path: target.to_str().unwrap().to_string(),
                mode: "existing".into(),
                branch: "main".into(),
                base: None,
                env_source: None,
            }),
        );
        let error = reply.result.unwrap_err();
        assert!(error.contains("checked out"), "{error}");
        assert!(!target.exists(), "a refused create left nothing behind");
    }

    // -- env preview retry latch (review item 3) ------------------------------

    fn env_form() -> Form {
        let mut f = form();
        f.copy_env = true;
        f.env_source = "A".into();
        f
    }

    #[test]
    fn queue_refusal_unlatches_env_request_so_retry_sends() {
        let mut s = State::new(crate::test_handle());
        s.editor.worktree_form = Some(env_form());
        // A dead receiver makes every send refuse.
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        request_env_preview(&mut s);
        let f = s.editor.worktree_form.as_ref().unwrap();
        assert!(f.env_error.is_some());
        assert_eq!(
            f.env_requested, None,
            "a refused request leaves no latch behind"
        );
        // Manual retry on a live lane issues the job again.
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        request_env_preview(&mut s);
        let f = s.editor.worktree_form.as_ref().unwrap();
        assert_eq!(f.env_requested.as_deref(), Some("A"));
        let job = rx.try_recv().expect("retry issued EnvCandidates");
        assert!(matches!(job.req, Req::EnvCandidates { .. }));
    }

    #[test]
    fn worker_error_unlatches_and_manual_retry_sends_again() {
        let mut s = State::new(crate::test_handle());
        s.editor.worktree_form = Some(env_form());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        request_env_preview(&mut s);
        let job = rx.try_recv().unwrap();
        s.editor.apply_reply(crate::editor::backend::Reply {
            req: job.req.clone(),
            generation: 0,
            checkout_key: None,
            result: Err("scan exploded".into()),
        });
        let f = s.editor.worktree_form.as_ref().unwrap();
        assert!(!f.env_loading);
        assert_eq!(f.env_error.as_deref(), Some("scan exploded"));
        assert_eq!(f.env_requested, None, "error releases the latch");
        // The same source retries only on a deliberate re-request — the
        // early return still guards an in-flight duplicate.
        request_env_preview(&mut s);
        assert!(rx.try_recv().is_ok(), "retry issued a fresh request");
        let in_flight = rx.try_recv().is_err();
        request_env_preview(&mut s);
        assert!(
            in_flight && rx.try_recv().is_err(),
            "a latched source cannot double-issue"
        );
    }

    #[test]
    fn stale_source_error_keeps_current_preview_and_loading() {
        let mut s = State::new(crate::test_handle());
        let mut f = env_form();
        f.env_source = "B".into();
        f.env_requested = Some("B".into());
        f.env_loading = true;
        f.env_files = Some(vec![EnvFileCandidate {
            name: ".env".into(),
            eligible: true,
            reason: None,
        }]);
        s.editor.worktree_form = Some(f);
        // An error for the previous source "A" must not touch "B"'s lane.
        s.editor.apply_reply(crate::editor::backend::Reply {
            req: Req::EnvCandidates {
                project_id: "p1".into(),
                source_path: "A".into(),
            },
            generation: 0,
            checkout_key: None,
            result: Err("A exploded".into()),
        });
        let f = s.editor.worktree_form.as_ref().unwrap();
        assert!(f.env_loading, "B's request still owns the spinner");
        assert!(f.env_error.is_none(), "A's error was dropped");
        assert_eq!(f.env_files.as_ref().unwrap().len(), 1);
        assert_eq!(f.env_requested.as_deref(), Some("B"));
    }

    // -- missing copy sources (review item 4) ---------------------------------

    #[test]
    fn missing_source_is_filtered_and_blocked_at_submit() {
        let mut gone = wt("gone", "/work/missing", false);
        gone.missing = true;
        let list = WorktreeList {
            repo: true,
            worktrees: vec![wt("web", "/work/web", true), gone],
        };
        // Desktop disables missing rows — they never reach the select.
        let options = source_options(&list.worktrees);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].1, "/work/web");
        // A stale selected source that went missing cannot submit a copy.
        let mut f = form();
        f.copy_env = true;
        f.env_source = "/work/missing".into();
        f.branches = Some(WorktreeBranches {
            branches: vec![branch("main", None)],
            bases: Vec::new(),
        });
        assert_eq!(
            validate(Some(&list), &f).as_deref(),
            Some("The copy source /work/missing is missing. Choose another checkout.")
        );
        // A present source validates fine (partial success stays the
        // backend's call for real copy failures).
        f.env_source = "/work/web".into();
        assert_eq!(validate(Some(&list), &f), None);
    }
}

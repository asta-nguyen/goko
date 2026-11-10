//! Navigation guards and once-only external open-file/line requests.
//!
//! Every path that would drop or hide unsaved work — opening another file,
//! switching checkout/project, changing workspace view or settings, closing
//! the window, quitting — goes through [`navigate`]. A dirty draft raises a
//! [`Guard`] (Save / Discard / Cancel); Save enqueues the write and resumes
//! the pending navigation only when it lands, Discard reverts the buffer,
//! Cancel drops the request. Failed saves keep the draft and cancel the
//! navigation, matching the desktop store's semantics.

use std::collections::HashSet;
use std::path::PathBuf;

use rune::Tx;

use crate::editor::backend::{self, Req};
use crate::editor::code::GoToRequest;
use crate::editor::go_to_definition as gtd;
use crate::editor::state::{EditorState, GoToPending, GoToStage, Notice, Section, checkout_key};
use crate::{State, View};

/// What a confirmed navigation should do once guards pass.
#[derive(Clone, Debug)]
pub enum PendingNav {
    /// Open `path` (checkout-relative) in the Files section, revealing `line`.
    OpenDoc { path: String, line: Option<u64> },
    /// Validated external open (OS open-file/URL): switch to `checkout`
    /// when its key differs, then open `path` there. Kept as ONE pending so
    /// the draft guard treats switch+open atomically — a Cancel drops both
    /// and a save-resume replays both, with the target identity intact.
    ExternalOpen {
        project_id: String,
        checkout: PathBuf,
        path: String,
        line: Option<u64>,
    },
    /// Switch project+checkout (validated before use).
    SelectCheckout {
        project_id: String,
        checkout: PathBuf,
    },
    /// Switch the selected project. Routed through the draft guard like
    /// every identity change: `project_id` only commits in `apply`, so a
    /// Cancel keeps BOTH the old project and its checkout/draft intact.
    SelectProject { project_id: String },
    /// Switch workspace section inside the editor.
    SetSection(Section),
    /// Jump to one Changes entry: select `path` in `layer`'s patch pane and
    /// land on the Changes section. Routed through the draft guard like
    /// every navigation — a Cancel leaves selection and section untouched.
    ShowChanges { layer: String, path: String },
    /// Switch the app-level view (Terminal / Editor / Settings).
    SetView(View),
    /// Close the window (red button / Cmd-W path).
    CloseWindow,
    /// Quit the app (Cmd-Q / Dock Quit after an OS-level veto).
    Quit,
}

/// A guard awaiting the user's Save/Discard/Cancel decision.
#[derive(Clone, Debug)]
pub struct Guard {
    pub pending: PendingNav,
    /// Document the guard protects (active doc at raise time).
    pub doc_path: String,
    /// `true` when raised by the OS quit veto rather than in-app navigation.
    pub quitting: bool,
}

/// Route `pending` through the draft guard, then apply it.
pub fn navigate(s: &mut State, pending: PendingNav, tx: &mut Tx) {
    if s.editor.guard.is_some() {
        // A guard is already up: replace its pending target? No — the dialog
        // is modal; ignore further navigation attempts until it resolves.
        return;
    }
    // Re-opening the file already open only reveals a line — it changes
    // nothing the guard protects, so it never prompts and never reloads.
    // (`apply`'s OpenDoc arm issues a fresh read, so the reveal must be
    // applied here directly.) An ExternalOpen qualifies only when its
    // checkout is the current one: a foreign checkout must still switch.
    let same_file = match &pending {
        PendingNav::OpenDoc { path, .. } => Some(path),
        PendingNav::ExternalOpen {
            project_id,
            checkout,
            path,
            ..
        } if s
            .editor
            .checkout_key
            .as_deref()
            .is_some_and(|key| key == checkout_key(project_id, checkout)) =>
        {
            Some(path)
        }
        _ => None,
    };
    if let Some(path) = same_file
        && let Some(doc) = s.editor.doc.as_mut()
        && doc.path == *path
    {
        let line = match &pending {
            PendingNav::OpenDoc { line, .. } | PendingNav::ExternalOpen { line, .. } => *line,
            _ => None,
        };
        if line.is_some() {
            doc.reveal_line = line;
        }
        s.editor.section = Section::Files;
        s.editor.visited.insert(Section::Files);
        return;
    }
    if let Some(doc) = s.editor.active_doc() {
        if doc.dirty() {
            s.editor.guard = Some(Guard {
                pending,
                doc_path: doc.path.clone(),
                quitting: false,
            });
            s.editor.sync_quit_dirty();
            return;
        }
        if doc.saving {
            // A save is in flight: hold navigation until it resolves rather
            // than race the write (the reply resumes or cancels).
            s.editor.save_resume = Some(Guard {
                pending,
                doc_path: doc.path.clone(),
                quitting: false,
            });
            return;
        }
    }
    apply(s, pending, tx);
}

/// Perform the navigation once guards have passed.
pub fn apply(s: &mut State, pending: PendingNav, tx: &mut Tx) {
    match pending {
        PendingNav::OpenDoc { path, line } => {
            // Guards already passed: issue the bounded read directly.
            // Routing back through `open_doc`→`navigate` would recurse.
            crate::editor::files::request_doc_load(s, &path, line);
        }
        PendingNav::ExternalOpen {
            project_id,
            checkout,
            path,
            line,
        } => {
            open_external(s, project_id, checkout, path, line);
        }
        PendingNav::SelectCheckout {
            project_id,
            checkout,
        } => {
            select_checkout(s, project_id, checkout);
        }
        PendingNav::SelectProject { project_id } => {
            select_project(s, project_id);
        }
        PendingNav::SetSection(section) => set_section(s, section),
        PendingNav::ShowChanges { layer, path } => {
            // Post-guard: point the patch pane at the entry, then activate
            // Changes — `set_section` issues a fresh `Req::Changes` so the
            // stage/unstage buttons reflect the latest data.
            crate::editor::changes::select_entry(&mut s.editor, &layer, &path);
            set_section(s, Section::Changes);
        }
        PendingNav::SetView(view) => {
            s.view = view;
            // Go-to-definition belongs to the Editor view's Files surface:
            // leaving the view cancels the lookup so a late reply cannot
            // pull the user back into the editor.
            if view != View::Editor {
                s.editor.go_to = None;
                // An in-flight ACP generate belongs to the editor's commit
                // panel — leaving the view abandons it.
                crate::editor::acp::cancel(s);
            } else {
                activate_sidebar_checkout(s, tx);
            }
        }
        PendingNav::CloseWindow => {
            // Post-guard: the close actually proceeds, so the editor's ACP
            // generate is abandoned with it.
            crate::editor::acp::cancel(s);
            tx.close_window();
        }
        PendingNav::Quit => {
            tx.quit();
        }
    }
}

/// Entering the Editor activates the checkout the sidebar last committed:
/// `active_workspace` decodes to (project, checkout); only a still-registered
/// project may drive the editor — mock session ids, `__unassigned__` and
/// vanished projects are ignored, and an already-matching selection is a
/// no-op. Routed through `navigate` so the draft guard treats it like every
/// other identity change (post-`SetView`-guard the doc is clean, so this
/// applies immediately).
fn activate_sidebar_checkout(s: &mut State, tx: &mut Tx) {
    let Some(workspace) = s.active_workspace.clone() else {
        return;
    };
    let Some((project_id, checkout_path)) = crate::data::checkout_context(&workspace) else {
        return;
    };
    if !s.editor.projects.iter().any(|p| p.project.id == project_id) {
        return;
    }
    let checkout = PathBuf::from(checkout_path);
    let key = checkout_key(project_id, &checkout);
    if s.editor.checkout_key.as_deref() == Some(key.as_str()) {
        return;
    }
    navigate(
        s,
        PendingNav::SelectCheckout {
            project_id: project_id.to_string(),
            checkout,
        },
        tx,
    );
}

/// Switch the visible section, marking it visited so its state persists.
pub fn set_section(s: &mut State, section: Section) {
    // Go-to-definition lookups belong to the Files code surface: leaving
    // it cancels the pending lookup so a late reply cannot navigate away
    // from wherever the user went.
    if section != Section::Files {
        s.editor.go_to = None;
    }
    // The Changes section hosts the ACP commit panel: leaving it abandons
    // any in-flight generate (uses the still-current identity).
    if s.editor.section == Section::Changes && section != Section::Changes {
        crate::editor::acp::cancel(s);
    }
    s.editor.section = section;
    s.editor.visited.insert(section);
    // Section reads are active-only: each section issues its own refresh on
    // activation and checkout change.
    crate::editor::workspace::refresh_active(s);
}

/// Select project+checkout after validation; clears per-checkout caches and
/// kicks the active section's reads. The caller must have validated through
/// `projects::git_checkout` semantics — the worker revalidates anyway.
pub fn select_checkout(s: &mut State, project_id: String, checkout: PathBuf) {
    let key = checkout_key(&project_id, &checkout);
    // The sidebar highlights the committed workspace identity — record it
    // here (post-guard) so a Cancel never moved it. `data::checkout_key`
    // uses the desktop's NUL separator; the editor key above keeps its own.
    s.active_workspace = Some(crate::data::checkout_key(
        &project_id,
        &checkout.display().to_string(),
    ));
    if s.editor.checkout_key.as_deref() == Some(key.as_str()) {
        return;
    }
    // An in-flight ACP generate belongs to the OLD checkout identity —
    // cancel before the mutation so its completion can never fill a draft
    // that now belongs somewhere else.
    crate::editor::acp::cancel(s);
    let project_changed = s.editor.project_id.as_deref() != Some(project_id.as_str());
    s.editor.project_id = Some(project_id.clone());
    if project_changed {
        // The checkout picker's worktree list belongs to the project:
        // reload it (and the sidebar's per-project entry) for the new
        // identity before the checkout-scoped reads kick below.
        s.editor.worktrees_error = None;
        s.editor.worktrees = app_core::worktrees::WorktreeList {
            repo: false,
            worktrees: Vec::new(),
        };
        if s.editor.request_worktrees(&project_id) {
            s.editor.worktrees_loading = true;
        } else {
            s.editor.worktrees_loading = false;
            s.editor.worktrees_error =
                Some("the editor queue is full; worktrees were not requested".into());
        }
    }
    s.editor.checkout = Some(checkout);
    s.editor.checkout_key = Some(key.clone());
    s.editor.pending_checkout = None;
    // Per-checkout state resets; the open doc belongs to the old checkout
    // and is dropped (its draft was guarded before this ran).
    s.editor.doc = None;
    s.editor.doc_load = None;
    s.editor.doc_reload = None;
    s.editor.save_resume = None;
    s.editor.files.model_checkout = None;
    s.editor.files.paths.clear();
    s.editor.changes = crate::editor::state::ChangesState::empty();
    s.editor.history.commits.clear();
    s.editor.history.next_skip = None;
    s.editor.history.selected = None;
    s.editor.history.detail = None;
    s.editor.history.checkout = None;
    s.editor.stashes.stashes.clear();
    s.editor.stashes.selected = None;
    s.editor.stashes.detail = None;
    s.editor.stashes.checkout = None;
    s.editor.go_to = None;
    s.editor.commit.message = rune::edit::TextEdit::new("");
    s.editor.commit.gen_snapshot = None;
    s.editor.commit.gen_checkout = None;
    s.editor.commit.generating = false;
    // In-flight commit/push replies are checkout-scoped and drop on the
    // staleness check — without this reset their flags would stay set in
    // the new checkout and deadlock Commit/Push for the session.
    s.editor.commit.committing = false;
    s.editor.commit.pushing = false;
    s.editor.commit.error = None;
    s.editor.notice = None;
    crate::editor::search::reset(s);
    s.editor.sync_quit_dirty();
    refresh_checkout_data(s);
}

/// Commit a project switch after the guard passed: update the identity,
/// fetch the new project's worktrees, and drop the checkout when it belongs
/// to the old project (its `checkout_key` prefix check is exact — keys are
/// `project_id\u{1f}canonical-path`).
pub fn select_project(s: &mut State, project_id: String) {
    if s.editor.project_id.as_deref() == Some(project_id.as_str()) {
        return;
    }
    // The project identity is about to change: cancel any in-flight ACP
    // generate under the OLD project+checkout before mutating either.
    crate::editor::acp::cancel(s);
    s.editor.project_id = Some(project_id.clone());
    s.editor.worktrees_error = None;
    s.editor.worktrees = app_core::worktrees::WorktreeList {
        repo: false,
        worktrees: Vec::new(),
    };
    // Same read the sidebar issues: the reply feeds both the picker and
    // the per-project sidebar entry (deduped while one is in flight).
    if s.editor.request_worktrees(&project_id) {
        s.editor.worktrees_loading = true;
    } else {
        s.editor.worktrees_loading = false;
        s.editor.worktrees_error =
            Some("the editor queue is full; worktrees were not requested".into());
    }
    let keep = s
        .editor
        .checkout_key
        .as_deref()
        .is_some_and(|key| key.starts_with(&format!("{project_id}\u{1f}")));
    if !keep {
        clear_checkout(s);
    }
}

/// Drop the checkout selection and every per-checkout cache (the shared tail
/// of `ClearCheckout` and a foreign-checkout project switch).
fn clear_checkout(s: &mut State) {
    // No checkout committed → nothing may stay highlighted as the active
    // workspace; the next SelectCheckout records the real key again.
    s.active_workspace = None;
    s.editor.checkout = None;
    s.editor.checkout_key = None;
    s.editor.pending_checkout = None;
    s.editor.doc = None;
    s.editor.doc_load = None;
    s.editor.doc_reload = None;
    s.editor.save_resume = None;
    s.editor.files.model_checkout = None;
    s.editor.files.paths.clear();
    s.editor.changes = crate::editor::state::ChangesState::empty();
    s.editor.history.commits.clear();
    s.editor.history.next_skip = None;
    s.editor.history.selected = None;
    s.editor.history.detail = None;
    s.editor.history.checkout = None;
    s.editor.stashes.stashes.clear();
    s.editor.stashes.selected = None;
    s.editor.stashes.detail = None;
    s.editor.stashes.checkout = None;
    s.editor.go_to = None;
    s.editor.commit.message = rune::edit::TextEdit::new("");
    s.editor.commit.gen_snapshot = None;
    s.editor.commit.gen_checkout = None;
    s.editor.commit.generating = false;
    s.editor.commit.committing = false;
    s.editor.commit.pushing = false;
    s.editor.commit.error = None;
    s.editor.notice = None;
    crate::editor::search::reset(s);
    s.editor.section = Section::Changes;
    s.editor.sync_quit_dirty();
}

/// Enqueue the reads the active section needs for the current checkout.
pub fn refresh_checkout_data(s: &mut State) {
    crate::editor::workspace::refresh_active(s);
}

/// Save/Discard/Cancel resolution for the visible guard.
pub fn resolve_guard(s: &mut State, action: GuardAction, tx: &mut Tx) {
    let Some(guard) = s.editor.guard.take() else {
        return;
    };
    match action {
        GuardAction::Cancel => {
            // Nothing changes; draft and pending navigation are dropped.
            // A quit veto is consumed either way: staying is the answer.
            s.editor.quit_vetoed = false;
            s.editor.sync_quit_dirty();
        }
        GuardAction::Discard => {
            if let Some(doc) = s.editor.active_doc_mut()
                && doc.path == guard.doc_path
            {
                doc.discard();
            }
            s.editor.sync_quit_dirty();
            apply(s, guard.pending, tx);
        }
        GuardAction::Save => {
            // Enqueue the save; the reply resumes `guard.pending` on success
            // or re-raises the error and drops it on failure.
            s.editor.save_resume = Some(guard);
            crate::editor::files::save_active_doc(s);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardAction {
    Save,
    Discard,
    Cancel,
}

/// Cancel without a `Tx` — the modal dialog's Escape/outside-press dismissal
/// (`El::modal`'s `Fn(&mut S)`) has no transaction to hand out. Identical to
/// `resolve_guard(Cancel)`: the guard and its pending navigation are dropped,
/// the draft stays, and a quit veto is consumed.
pub fn cancel_guard(s: &mut State) {
    if s.editor.guard.take().is_some() {
        s.editor.quit_vetoed = false;
        s.editor.sync_quit_dirty();
    }
}

/// `App::on_close` / `AppHandle::close` path: a dirty draft cancels the close.
/// Rune's close is not a veto, so the window hides and `poll` reopens it once
/// the guard resolves (or immediately when the draft turned out clean).
pub fn on_window_close(s: &mut State, tx: &mut Tx) {
    let dirty = s.editor.has_dirty_doc();
    s.editor.sync_quit_dirty();
    if !dirty {
        // Clean: also flush an in-flight save's resume? Nothing pending — let
        // the close proceed; workers finish their writes.
        return;
    }
    if let Some(doc) = s.editor.active_doc() {
        s.editor.guard = Some(Guard {
            pending: PendingNav::CloseWindow,
            doc_path: doc.path.clone(),
            quitting: false,
        });
    }
    s.editor.reopen_after_close = true;
    let _ = tx;
}

/// `poll` hook: after a cancelled close hid the window, reopen it so the
/// guard is visible. Runs every poll while the flag is set; clears once the
/// window is requested again.
pub fn reopen_if_cancelled(s: &mut State) {
    if !s.editor.reopen_after_close {
        return;
    }
    if s.editor.guard.is_some() {
        if let Some(app) = &s.editor.app {
            app.open();
        }
        s.editor.reopen_after_close = false;
    } else {
        // Guard resolved while hidden (e.g. Discard applied CloseWindow, or
        // Cancel left the draft dirty — the window should come back either
        // way so the user sees the state they cancelled into).
        s.editor.reopen_after_close = false;
        if let Some(app) = &s.editor.app {
            app.open();
        }
    }
}

/// The OS quit veto (macOS `applicationShouldTerminate` swizzle, see
/// quit_guard) landed: surface the guard as a Quit prompt.
pub fn on_quit_vetoed(s: &mut State) {
    if !s.editor.has_dirty_doc() {
        // Stale flag: nothing to protect — let the quit continue.
        if let Some(app) = s.editor.app.clone() {
            app.quit();
        }
        return;
    }
    if let Some(doc) = s.editor.active_doc() {
        s.editor.guard = Some(Guard {
            pending: PendingNav::Quit,
            doc_path: doc.path.clone(),
            quitting: true,
        });
    }
    s.editor.quit_vetoed = true;
    s.editor.sync_quit_dirty();
    // The window never closed, but make sure it's frontmost for the dialog.
    if let Some(app) = &s.editor.app {
        app.open();
    }
}

/// Discard chosen for a quit: the draft dies with the process, so clearing
/// the dirty flag and re-issuing quit is the honest equivalent.
pub fn quit_discard(s: &mut State) {
    if let Some(doc) = s.editor.doc.as_mut() {
        doc.discard();
    }
    s.editor.sync_quit_dirty();
    s.editor.quit_vetoed = false;
    if let Some(app) = &s.editor.app {
        app.quit();
    }
}

/// Save chosen for a quit: the save reply sees `PendingNav::Quit` and calls
/// `handle.quit()` once the buffer is clean — `should_terminate` then finds
/// no dirty flag and lets Rune's original hook terminate.
pub fn quit_after_save(s: &mut State) {
    if s.editor.doc.is_some() {
        if let Some(doc) = s.editor.active_doc() {
            s.editor.save_resume = Some(Guard {
                pending: PendingNav::Quit,
                doc_path: doc.path.clone(),
                quitting: true,
            });
        }
        s.editor.guard = None;
        crate::editor::files::save_active_doc(s);
    } else {
        if let Some(app) = &s.editor.app {
            app.quit();
        }
    }
}

/// External open-file/line request (main's `editorOpenRequest` port): the
/// request goes to the backend worker, which validates project + checkout
/// through `projects::git_checkout` — no storage reads on the UI thread —
/// and applies the result once via `apply_external_open`.
///
/// No in-app caller yet: the OS-level producer (launch arg / open-URL
/// event) lands with the live-terminal milestone. The consumer — path
/// validation, once-only application, wrong-checkout rejection — is real
/// and tested now, so the seam stays `pub` for that producer.
#[allow(dead_code)]
pub fn external_open(
    s: &mut State,
    project_id: &str,
    checkout_path: &str,
    path: &str,
    line: Option<u64>,
    _tx: &mut Tx,
) {
    if path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|seg| seg == "..")
        || path == "."
    {
        return; // malformed paths are rejected, not guessed at
    }
    s.editor
        .backend
        .send(backend::Job::project(backend::Req::ExternalOpen {
            project_id: project_id.to_string(),
            checkout_path: checkout_path.to_string(),
            path: path.to_string(),
            line,
        }));
}

/// `PendingNav::ExternalOpen` application — post-guard, so the draft is
/// safe. Switch checkout when the worker-validated key differs from the
/// current one, then open the file in the (possibly new) checkout.
fn open_external(
    s: &mut State,
    project_id: String,
    checkout: PathBuf,
    path: String,
    line: Option<u64>,
) {
    let key = checkout_key(&project_id, &checkout);
    if s.editor.checkout_key.as_deref() != Some(key.as_str()) {
        select_checkout(s, project_id, checkout);
    }
    // Same-file reveal without a reload — the guard already passed, so this
    // mirrors `navigate`'s OpenDoc shortcut rather than recursing through it.
    if let Some(doc) = s.editor.doc.as_mut()
        && doc.path == path
    {
        if line.is_some() {
            doc.reveal_line = line;
        }
        s.editor.section = Section::Files;
        s.editor.visited.insert(Section::Files);
        return;
    }
    crate::editor::files::request_doc_load(s, &path, line);
}

/// Issue the "reload from disk" read for the open doc — the guard dialog's
/// escape hatch after a failed save. The request carries the doc's current
/// `CodeState` revision: any edit during the flight bumps it and the reply
/// drops, so a reload can never clobber newer typing. The UI owner wires
/// this to the dialog button; the worker/apply halves live in `backend`.
#[allow(dead_code)] // caller lands with the guard dialog's Reload button
pub fn request_doc_reload(s: &mut State) {
    let Some(doc) = s.editor.doc.as_ref() else {
        return;
    };
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    let path = doc.path.clone();
    let revision = doc.code.revision;
    s.editor.doc_seq += 1;
    let token = s.editor.doc_seq;
    if s.editor.enqueue_read(
        checkout,
        key,
        Req::ReloadFile {
            path,
            token,
            revision,
        },
    ) {
        s.editor.doc_reload = Some(token);
    } else {
        s.editor.notice = Some(Notice {
            text: "the editor queue is full; reload was not sent".into(),
            stale: true,
        });
    }
}

/// Write requests always serialize on the single write lane; `after` carries
/// the navigation a successful save should resume. A refused enqueue clears
/// `saving` immediately so the doc never sticks on "save in flight".
pub fn enqueue_save(s: &mut State) {
    let Some(doc) = s.editor.doc.as_ref() else {
        return;
    };
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    let Some(pid) = s.editor.project_id.clone() else {
        return;
    };
    let path = doc.path.clone();
    let expected = doc.original.clone();
    // The payload is the newline-normalized save text — the exact bytes
    // that hit disk — and it gets the same caps the raw buffer did:
    // normalization can grow it past the limit.
    let new_text = doc.text_for_save();
    if new_text.len() > crate::editor::draft::TEXT_CAP {
        if let Some(doc) = s.editor.doc.as_mut() {
            doc.save_error = Some("the file exceeds the 1 MiB save limit".into());
        }
        s.editor.save_resume = None;
        s.editor.sync_quit_dirty();
        return;
    }
    if new_text.contains('\0') {
        if let Some(doc) = s.editor.doc.as_mut() {
            doc.save_error = Some("the file contains a NUL byte".into());
        }
        s.editor.save_resume = None;
        s.editor.sync_quit_dirty();
        return;
    }
    let ok = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::write(
            &pid,
            checkout,
            key,
            backend::Req::SaveFile {
                path,
                expected,
                text: new_text,
            },
        ),
    );
    if let Some(doc) = s.editor.doc.as_mut() {
        doc.saving = ok;
        doc.save_error = if ok {
            None
        } else {
            Some("the editor queue is full; save not sent".into())
        };
        if ok {
            // Freeze edits while the write is in flight: the buffer must
            // stay identical to the text being saved. `read_only` only
            // changes input handling — the TextEdit model and its undo
            // history are untouched, and `finish_save` restores the flag
            // on every outcome (success, failure, stale doc).
            doc.code.read_only = true;
        }
    }
    if !ok {
        // The navigation this save was resuming must not hang waiting for a
        // reply that will never come.
        s.editor.save_resume = None;
    }
    s.editor.sync_quit_dirty();
}

// ---------------------------------------------------------------------------
// Go-to-definition: the code owner's pure resolver (`go_to_definition.rs`)
// plus the two backend reads it needs — nearest tsconfig/jsconfig `paths`
// aliases for imports, and a dedicated content search for identifiers.
// ---------------------------------------------------------------------------

/// `Code::on_go_to` entry point (modifier-held glyph link clicked, or F12).
/// A live language server with definition support answers first; its
/// reply resolves through the guarded pending-nav path or queues the
/// heuristic fallback itself. Without one, the heuristic pipeline runs
/// immediately.
pub fn start_go_to(s: &mut State, req: GoToRequest, tx: &mut Tx) {
    if s.editor.lsp_try_definition(&req, tx) {
        return;
    }
    start_go_to_heuristic(s, req, tx);
}

/// The pre-LSP pipeline: tsconfig/jsconfig `paths` aliases for imports,
/// then a bounded content search for identifiers.
pub fn start_go_to_heuristic(s: &mut State, req: GoToRequest, tx: &mut Tx) {
    let Some(from_path) = s.editor.active_doc().map(|d| d.path.clone()) else {
        return;
    };
    match &req.target {
        gtd::GoToTarget::Import(specifier) => {
            let files: HashSet<String> = s.editor.files.paths.iter().cloned().collect();
            // Relative specifiers (`./x`, `../x`) resolve without any config.
            if let Some(hit) = gtd::resolve_import(&files, &from_path, specifier, &[]) {
                navigate(
                    s,
                    PendingNav::OpenDoc {
                        path: hit,
                        line: None,
                    },
                    tx,
                );
                return;
            }
            // Bare/aliased specifier: ONE config lookup reads the ordered
            // tsconfig/jsconfig candidates (nearest first, no depth cap —
            // same list the desktop walks) for `paths` aliases, then retry.
            let candidates = gtd::config_candidates(&from_path);
            if candidates.is_empty() {
                s.editor.notice = Some(Notice {
                    text: format!("no definition found for '{specifier}'"),
                    stale: false,
                });
                return;
            }
            let specifier = specifier.clone();
            s.editor.go_to = Some(GoToPending {
                from_path,
                stage: GoToStage::AwaitConfigs { specifier },
            });
            enqueue_config_lookup(&mut s.editor, candidates);
        }
        gtd::GoToTarget::Name(name) => {
            let name = name.clone();
            s.editor.go_to = Some(GoToPending {
                from_path,
                stage: GoToStage::SearchName { name: name.clone() },
            });
            let Some((checkout, key)) =
                s.editor.checkout.clone().zip(s.editor.checkout_key.clone())
            else {
                s.editor.go_to = None;
                return;
            };
            if !s
                .editor
                .enqueue_read(checkout, key, Req::GoToSearch { query: name })
            {
                s.editor.go_to = None;
                s.editor.notice = Some(Notice {
                    text: "the editor queue is full; go-to-definition was not sent".into(),
                    stale: true,
                });
            }
        }
    }
}

/// Issue the single config lookup for an in-flight import resolution: one
/// `ConfigFiles` read carries every ordered candidate, so it gets exactly
/// one generation — a second lookup supersedes it and drops its reply.
fn enqueue_config_lookup(s: &mut EditorState, candidates: Vec<String>) {
    let Some(checkout) = s.checkout.clone() else {
        s.go_to = None;
        return;
    };
    let Some(key) = s.checkout_key.clone() else {
        s.go_to = None;
        return;
    };
    if !s.enqueue_read(checkout, key, Req::ConfigFiles { paths: candidates }) {
        s.go_to = None;
        s.notice = Some(Notice {
            text: "the editor queue is full; go-to-definition was not sent".into(),
            stale: true,
        });
    }
}

/// The config-lookup reply: aliases already parsed worker-side from the
/// nearest config that defines any. The source doc must still be the one
/// the request came from — a switched doc makes the answer stale (the
/// desktop's `scope` check).
pub fn go_to_config_reply(s: &mut EditorState, aliases: Vec<gtd::PathAlias>) {
    let Some(pending) = s.go_to.take() else {
        return;
    };
    if !matches!(pending.stage, GoToStage::AwaitConfigs { .. }) {
        s.go_to = Some(pending);
        return;
    }
    // The source doc moved on (closed or switched) — the aliases answer a
    // request that no longer has a live origin, so drop it entirely.
    if s.doc.as_ref().map(|d| d.path.as_str()) != Some(pending.from_path.as_str()) {
        return;
    }
    // Direct `section` assignments bypass `set_section`'s cancel: the
    // lookup belongs to the Files surface, so a reply arriving after the
    // user left must be consumed without navigating.
    if s.section != Section::Files {
        return;
    }
    finish_import_resolution(s, pending, aliases);
}

/// Retry `resolve_import` with the harvested aliases and either queue the
/// doc open or report the miss.
fn finish_import_resolution(
    s: &mut EditorState,
    pending: GoToPending,
    aliases: Vec<gtd::PathAlias>,
) {
    let GoToStage::AwaitConfigs { specifier } = pending.stage else {
        return;
    };
    let files: HashSet<String> = s.files.paths.iter().cloned().collect();
    if let Some(hit) = gtd::resolve_import(&files, &pending.from_path, &specifier, &aliases) {
        s.pending_apply = Some(PendingNav::OpenDoc {
            path: hit,
            line: None,
        });
    } else {
        s.notice = Some(Notice {
            text: format!("no definition found for '{specifier}'"),
            stale: false,
        });
    }
}

/// The `GoToSearch` reply: pick the unique declaration and queue the doc
/// open through the draft guard; no unique hit falls back to showing the
/// search pane with the same results (desktop behaviour).
pub fn go_to_search_reply(
    s: &mut EditorState,
    query: String,
    result: app_core::git_view::GitViewSearch,
) {
    let Some(pending) = s.go_to.take() else {
        return;
    };
    let GoToStage::SearchName { name } = pending.stage.clone() else {
        s.go_to = Some(pending);
        return;
    };
    if name != query {
        s.go_to = Some(pending);
        return;
    }
    // Same section gate as `go_to_config_reply`: the search pane and doc
    // open it can produce are Files-surface navigation.
    if s.section != Section::Files {
        return;
    }
    let matches: Vec<gtd::SearchMatch> = result
        .matches
        .iter()
        .map(|m| gtd::SearchMatch {
            path: m.path.clone(),
            line: m.line as u32,
            text: m.text.clone(),
        })
        .collect();
    if let Some(def) = gtd::pick_definition(&matches, &name, &pending.from_path) {
        s.pending_apply = Some(PendingNav::OpenDoc {
            path: def.path,
            line: Some(def.line as u64),
        });
        return;
    }
    // Ambiguous or absent: surface the same result set in the search pane.
    s.files.search.open = true;
    s.files.search.query = name;
    s.files.search.results = Some(result);
    s.files.search.searching = false;
    s.files.search.error = None;
    s.files.search.checkout = s.checkout_key.clone();
    s.section = Section::Files;
    s.visited.insert(Section::Files);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::state::checkout_key;
    use app_core::git_view::GitViewFile;
    use std::path::PathBuf;

    fn state() -> State {
        State::new(crate::test_handle())
    }

    /// Registered project + checkout, as `select_checkout` leaves them.
    fn checkout_state() -> State {
        let mut s = state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/tmp/co"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        s
    }

    fn file(text: &str) -> GitViewFile {
        GitViewFile {
            path: "a.txt".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    /// Install `a.txt` with `edited` diverging from `orig` → dirty draft.
    fn dirty_doc(s: &mut State) {
        let mut doc = OpenDoc::new("a.txt", &file("orig"));
        doc.code.edit.text = "edited".into();
        s.editor.doc = Some(doc);
    }

    #[test]
    fn project_switch_cancel_preserves_identity_and_draft() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectProject {
                project_id: "p2".into(),
            },
            &mut tx,
        );
        // The guard is up and NOTHING committed: project, checkout and the
        // dirty draft all still belong to p1.
        assert!(s.editor.guard.is_some());
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert!(s.editor.checkout_key.is_some());
        assert!(s.editor.doc.as_ref().unwrap().dirty());
        resolve_guard(&mut s, GuardAction::Cancel, &mut tx);
        assert!(s.editor.guard.is_none());
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert!(s.editor.checkout_key.is_some());
        assert!(s.editor.doc.as_ref().unwrap().dirty());
    }

    #[test]
    fn project_switch_discard_commits_and_clears_foreign_checkout() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectProject {
                project_id: "p2".into(),
            },
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Discard, &mut tx);
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        // The p1 checkout key cannot belong to p2 → selection cleared.
        assert!(s.editor.checkout.is_none());
        assert!(s.editor.checkout_key.is_none());
        assert!(s.editor.doc.is_none());
    }

    #[test]
    fn project_switch_failed_save_keeps_everything() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        // A NUL byte makes validate_save fail deterministically — no worker
        // round-trip needed to prove the failed-save path.
        s.editor.doc.as_mut().unwrap().code.edit.text = "a\0b".into();
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectProject {
                project_id: "p2".into(),
            },
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Save, &mut tx);
        // Save rejected → draft retained, navigation cancelled, identity kept.
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert!(s.editor.checkout_key.is_some());
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(doc.dirty());
        assert!(doc.save_error.is_some());
        assert!(s.editor.save_resume.is_none());
        assert!(s.editor.guard.is_none());
    }

    #[test]
    fn same_file_open_reveals_without_guard_or_reload() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        s.editor.doc.as_mut().unwrap().reveal_line = None;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::OpenDoc {
                path: "a.txt".into(),
                line: Some(3),
            },
            &mut tx,
        );
        // Same file + dirty draft: no guard, no reload token, just a reveal.
        assert!(s.editor.guard.is_none());
        assert_eq!(s.editor.doc.as_ref().unwrap().reveal_line, Some(3));
        assert!(s.editor.doc_load.is_none());
        assert!(s.editor.doc.as_ref().unwrap().dirty());
        assert_eq!(s.editor.section, Section::Files);
    }

    #[test]
    fn other_file_open_raises_guard_on_dirty_draft() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::OpenDoc {
                path: "b.txt".into(),
                line: None,
            },
            &mut tx,
        );
        let guard = s.editor.guard.as_ref().unwrap();
        assert_eq!(guard.doc_path, "a.txt");
        assert!(matches!(guard.pending, PendingNav::OpenDoc { ref path, .. } if path == "b.txt"));
        // The draft is untouched while the dialog is up.
        assert!(s.editor.doc.as_ref().unwrap().dirty());
    }

    #[test]
    fn clean_doc_section_switch_applies_immediately() {
        let mut s = checkout_state();
        // Non-dirty doc: navigation proceeds without a guard.
        s.editor.doc = Some(OpenDoc::new("a.txt", &file("orig")));
        let mut tx = Tx::default();
        navigate(&mut s, PendingNav::SetSection(Section::History), &mut tx);
        assert!(s.editor.guard.is_none());
        assert_eq!(s.editor.section, Section::History);
    }

    #[test]
    fn external_open_rejects_traversal_and_absolute_paths() {
        let mut s = checkout_state();
        let mut tx = Tx::default();
        for bad in ["../etc/passwd", "a/../../b", "/abs/path", ".", ""] {
            external_open(&mut s, "p1", "/tmp/co", bad, None, &mut tx);
        }
        // Rejected requests queue nothing and touch no state.
        assert!(s.editor.pending_apply.is_none());
        assert!(s.editor.guard.is_none());
        assert!(s.editor.doc.is_none());
    }

    #[test]
    fn quit_veto_then_discard_allows_quit() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        on_quit_vetoed(&mut s);
        let guard = s.editor.guard.as_ref().unwrap();
        assert!(guard.quitting);
        assert!(matches!(guard.pending, PendingNav::Quit));
        assert!(s.editor.quit_vetoed);
        quit_discard(&mut s);
        assert!(!s.editor.quit_vetoed);
        assert!(!s.editor.doc.as_ref().unwrap().dirty());
    }

    #[test]
    fn window_close_guard_reopens_after_cancel() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        on_window_close(&mut s, &mut tx);
        assert!(s.editor.reopen_after_close);
        assert!(matches!(
            s.editor.guard.as_ref().unwrap().pending,
            PendingNav::CloseWindow
        ));
        resolve_guard(&mut s, GuardAction::Cancel, &mut tx);
        // poll's reopen path clears the flag and re-shows the window.
        reopen_if_cancelled(&mut s);
        assert!(!s.editor.reopen_after_close);
    }

    #[test]
    fn go_to_import_issues_one_config_lookup() {
        let mut s = checkout_state();
        s.editor.doc = Some(OpenDoc::new("src/a.ts", &file("x")));
        s.editor.files.paths = vec![
            "src/a.ts".into(),
            "src/app/util.ts".into(),
            "tsconfig.json".into(),
        ];
        let req = GoToRequest {
            target: gtd::GoToTarget::Import("@app/util".into()),
            from: 0,
            to: 5,
        };
        start_go_to(&mut s, req, &mut Tx::default());
        assert!(
            matches!(
                s.editor.go_to.as_ref().map(|p| &p.stage),
                Some(GoToStage::AwaitConfigs { .. })
            ),
            "bare import parks an alias lookup"
        );
        // ONE worker request carries the whole ordered candidate list —
        // the old per-file fan-out bumped the shared generation once per
        // candidate, so every reply but the last was dropped and the
        // pending count could never drain.
        assert_eq!(
            s.editor.req_gens.get("configfiles").copied(),
            Some(1),
            "exactly one config lookup generation"
        );
    }

    fn import_state() -> State {
        let mut s = checkout_state();
        // Go-to lookups only originate on the Files code surface.
        s.editor.section = Section::Files;
        s.editor.doc = Some(OpenDoc::new("src/a.ts", &file("x")));
        s.editor.files.paths = vec![
            "src/a.ts".into(),
            "src/app/util.ts".into(),
            "src/app/other.ts".into(),
        ];
        s
    }

    fn import_req(specifier: &str) -> GoToRequest {
        GoToRequest {
            target: gtd::GoToTarget::Import(specifier.into()),
            from: 0,
            to: 5,
        }
    }

    fn alias(pattern: &str, target: &str) -> gtd::PathAlias {
        gtd::PathAlias {
            pattern: pattern.into(),
            targets: vec![target.into()],
        }
    }

    fn config_reply(
        generation: u64,
        key: Option<String>,
        aliases: Vec<gtd::PathAlias>,
    ) -> backend::Reply {
        backend::Reply {
            req: Req::ConfigFiles {
                paths: vec!["tsconfig.json".into()],
            },
            generation,
            checkout_key: key,
            result: Ok(backend::ReplyData::ConfigAliases(aliases)),
        }
    }

    #[test]
    fn go_to_import_reply_resolves_via_aliases() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        go_to_config_reply(&mut s.editor, vec![alias("@app/*", "src/app/*")]);
        assert!(s.editor.go_to.is_none(), "lookup consumed");
        assert!(
            matches!(
                s.editor.pending_apply,
                Some(PendingNav::OpenDoc { ref path, .. }) if path == "src/app/util.ts"
            ),
            "alias resolved: {:?}",
            s.editor.pending_apply
        );
    }

    #[test]
    fn go_to_second_lookup_supersedes_first_reply() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        // A second import lookup bumps the configfiles generation to 2.
        start_go_to(&mut s, import_req("@app/other"), &mut Tx::default());
        assert_eq!(s.editor.req_gens.get("configfiles").copied(), Some(2));
        let key = s.editor.checkout_key.clone();
        // The first lookup's gen-1 reply drops: nothing opens, the newer
        // pending survives for its own reply.
        s.editor.apply_reply(config_reply(
            1,
            key.clone(),
            vec![alias("@app/*", "src/app/*")],
        ));
        assert!(s.editor.pending_apply.is_none(), "stale reply dropped");
        assert!(s.editor.go_to.is_some(), "newer lookup still pending");
        // The current generation resolves the SECOND specifier.
        s.editor
            .apply_reply(config_reply(2, key, vec![alias("@app/*", "src/app/*")]));
        assert!(
            matches!(
                s.editor.pending_apply,
                Some(PendingNav::OpenDoc { ref path, .. }) if path == "src/app/other.ts"
            ),
            "second lookup resolved: {:?}",
            s.editor.pending_apply
        );
    }

    #[test]
    fn go_to_checkout_switch_drops_pending_lookup() {
        let mut s = import_state();
        let old_key = s.editor.checkout_key.clone();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        assert!(s.editor.go_to.is_some());
        select_checkout(&mut s, "p1".into(), PathBuf::from("/tmp/other"));
        assert!(s.editor.go_to.is_none(), "checkout switch clears pending");
        // A late reply scoped to the old checkout cannot navigate either.
        s.editor
            .apply_reply(config_reply(1, old_key, vec![alias("@app/*", "src/app/*")]));
        assert!(s.editor.pending_apply.is_none());
    }

    #[test]
    fn go_to_doc_switch_drops_stale_aliases() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        // The source doc changed before the reply landed (desktop `scope`).
        s.editor.doc = Some(OpenDoc::new("b.ts", &file("y")));
        go_to_config_reply(&mut s.editor, vec![alias("@app/*", "src/app/*")]);
        assert!(s.editor.pending_apply.is_none(), "stale origin dropped");
        assert!(s.editor.go_to.is_none(), "pending consumed, not restored");
    }

    #[test]
    fn go_to_pending_cancelled_when_leaving_files_section() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        assert!(s.editor.go_to.is_some());
        set_section(&mut s, Section::Changes);
        assert!(s.editor.go_to.is_none(), "section leave cancels lookup");
    }

    #[test]
    fn go_to_reply_consumed_without_nav_after_direct_section_switch() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        // Direct `section` writes bypass `set_section` — the reply-side
        // gate is the backstop: consume the pending, navigate nowhere.
        s.editor.section = Section::History;
        go_to_config_reply(&mut s.editor, vec![alias("@app/*", "src/app/*")]);
        assert!(s.editor.go_to.is_none());
        assert!(s.editor.pending_apply.is_none());
        assert!(s.editor.notice.is_none());
    }

    #[test]
    fn go_to_pending_cancelled_when_leaving_editor_view() {
        let mut s = import_state();
        start_go_to(&mut s, import_req("@app/util"), &mut Tx::default());
        navigate(
            &mut s,
            PendingNav::SetView(View::Terminal),
            &mut Tx::default(),
        );
        assert_eq!(s.view, View::Terminal);
        assert!(s.editor.go_to.is_none(), "view leave cancels lookup");
    }

    // -- external open -------------------------------------------------------

    fn external_pending(checkout: &str, path: &str, line: Option<u64>) -> PendingNav {
        PendingNav::ExternalOpen {
            project_id: "p1".into(),
            checkout: PathBuf::from(checkout),
            path: path.into(),
            line,
        }
    }

    #[test]
    fn external_open_switches_checkout_and_issues_doc_load() {
        let mut s = checkout_state();
        s.editor.section = Section::Files;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            external_pending("/tmp/other", "b.ts", Some(2)),
            &mut tx,
        );
        // Clean state: applied immediately — checkout switched and the
        // file read armed against the NEW checkout.
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/tmp/other"))
        );
        assert!(s.editor.doc.is_none(), "old checkout's doc dropped");
        assert!(s.editor.doc_load.is_some(), "doc read armed");
    }

    #[test]
    fn external_open_guard_cancel_never_opens_in_old_checkout() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            external_pending("/tmp/other", "b.ts", None),
            &mut tx,
        );
        // The switch+open is ONE guarded pending.
        assert!(s.editor.guard.is_some());
        assert!(
            matches!(
                s.editor.guard.as_ref().unwrap().pending,
                PendingNav::ExternalOpen { .. }
            ),
            "guard carries the atomic external open"
        );
        resolve_guard(&mut s, GuardAction::Cancel, &mut tx);
        // Cancel drops BOTH halves: no switch, no open — and nothing left
        // behind that a later poll could fire against the old checkout
        // (the pending slot was consumed by the guard itself).
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/tmp/co"))
        );
        assert!(s.editor.doc.is_some(), "dirty draft preserved");
        assert!(s.editor.doc_load.is_none(), "no file read armed");
        assert!(s.editor.pending_apply.is_none());
    }

    #[test]
    fn external_open_same_file_reveals_line_without_guard_or_reload() {
        let mut s = checkout_state();
        let mut doc = OpenDoc::new("a.txt", &file("orig"));
        doc.code.edit.text = "edited".into(); // dirty: reveal must not prompt
        s.editor.doc = Some(doc);
        s.editor.section = Section::Changes;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            external_pending("/tmp/co", "a.txt", Some(7)),
            &mut tx,
        );
        assert!(s.editor.guard.is_none(), "same-file reveal never prompts");
        let doc = s.editor.doc.as_ref().unwrap();
        assert_eq!(doc.reveal_line, Some(7));
        assert_eq!(s.editor.section, Section::Files);
        assert!(s.editor.doc_load.is_none(), "no reload issued");
        assert!(doc.dirty(), "draft untouched");
    }

    #[test]
    fn checkout_switch_resets_commit_write_flags() {
        let mut s = checkout_state();
        s.editor.commit.committing = true;
        s.editor.commit.pushing = true;
        let old_key = s.editor.checkout_key.clone().unwrap();
        select_checkout(&mut s, "p1".into(), PathBuf::from("/tmp/other"));
        // In-flight write replies are checkout-scoped and drop on the
        // staleness check — the flags must reset here or Commit/Push stay
        // dead in the new checkout for the rest of the session.
        assert!(!s.editor.commit.committing);
        assert!(!s.editor.commit.pushing);
        // The stale reply lands scope-dropped: it neither clears nor
        // resurrects flags belonging to the new checkout.
        s.editor.apply_reply(backend::Reply {
            req: Req::Commit {
                message: "m".into(),
            },
            generation: 0,
            checkout_key: Some(old_key),
            result: Ok(backend::ReplyData::Commit(
                app_core::git_write::GitCommitResult {
                    head: "abc1234".into(),
                    branch: Some("main".into()),
                },
            )),
        });
        assert!(!s.editor.commit.committing);
        assert!(!s.editor.commit.pushing);
        assert!(s.editor.notice.is_none());
    }

    #[test]
    fn project_switch_clears_commit_write_flags() {
        let mut s = checkout_state();
        s.editor.commit.committing = true;
        s.editor.commit.pushing = true;
        // p2's checkout key can't prefix-match p1's → clear_checkout runs.
        select_project(&mut s, "p2".into());
        assert!(!s.editor.commit.committing);
        assert!(!s.editor.commit.pushing);
        assert!(s.editor.checkout.is_none());
    }

    #[test]
    fn external_open_guard_save_resumes_switch_and_open() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            external_pending("/tmp/other", "b.ts", None),
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Save, &mut tx);
        // Save enqueued; the reply's finish_save resumes the WHOLE
        // ExternalOpen pending — switch and open replay together.
        assert!(s.editor.doc.as_ref().unwrap().saving);
        assert!(matches!(
            s.editor.save_resume.as_ref().unwrap().pending,
            PendingNav::ExternalOpen { .. }
        ));
    }

    // -- PendingNav::ShowChanges ---------------------------------------------

    #[test]
    fn show_changes_cancel_leaves_section_and_selection_untouched() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        s.editor.section = Section::Files;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::ShowChanges {
                layer: "unstaged".into(),
                path: "a.txt".into(),
            },
            &mut tx,
        );
        // Dirty draft → the guard holds the pending; nothing applied.
        assert!(matches!(
            s.editor.guard.as_ref().unwrap().pending,
            PendingNav::ShowChanges { .. }
        ));
        assert_eq!(s.editor.section, Section::Files);
        assert!(s.editor.changes.patch_path.is_none());
        resolve_guard(&mut s, GuardAction::Cancel, &mut tx);
        assert_eq!(s.editor.section, Section::Files);
        assert!(s.editor.changes.patch_path.is_none());
        assert!(s.editor.changes.layer.is_none());
    }

    #[test]
    fn show_changes_discard_selects_entry_and_activates_changes() {
        let mut s = checkout_state();
        dirty_doc(&mut s);
        s.editor.section = Section::Files;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::ShowChanges {
                layer: "unstaged".into(),
                path: "a.txt".into(),
            },
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Discard, &mut tx);
        assert_eq!(s.editor.section, Section::Changes);
        assert_eq!(s.editor.changes.layer.as_deref(), Some("unstaged"));
        assert_eq!(s.editor.changes.patch_path.as_deref(), Some("a.txt"));
        assert_eq!(s.editor.changes.selected.as_deref(), Some("a.txt"));
        // `set_section` kicked the Changes refresh — a fresh Req::Changes
        // generation exists so stage/unstage state is current.
        assert!(s.editor.req_gens.get("changes").copied().unwrap_or(0) > 0);
    }

    #[test]
    fn show_changes_clean_doc_applies_immediately() {
        let mut s = checkout_state();
        s.editor.doc = Some(OpenDoc::new("a.txt", &file("orig")));
        s.editor.section = Section::Files;
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::ShowChanges {
                layer: "staged".into(),
                path: "b.rs".into(),
            },
            &mut tx,
        );
        assert!(s.editor.guard.is_none());
        assert_eq!(s.editor.section, Section::Changes);
        assert_eq!(s.editor.changes.patch_path.as_deref(), Some("b.rs"));
    }

    // -- Save read_only freeze ------------------------------------------------

    fn dirty_editable_doc(s: &mut State) {
        let mut doc = OpenDoc::new("a.txt", &file("orig"));
        doc.code.edit.text = "edited".into();
        doc.code.text_edited();
        s.editor.doc = Some(doc);
    }

    fn save_reply(
        s: &State,
        result: Result<app_core::git_view::GitViewFile, String>,
    ) -> backend::Reply {
        backend::Reply {
            req: backend::Req::SaveFile {
                path: "a.txt".into(),
                expected: "orig".into(),
                text: "edited".into(),
            },
            generation: 0,
            checkout_key: s.editor.checkout_key.clone(),
            result: result.map(backend::ReplyData::Saved),
        }
    }

    #[test]
    fn save_freezes_read_only_and_restores_on_success() {
        let mut s = checkout_state();
        dirty_editable_doc(&mut s);
        enqueue_save(&mut s);
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(doc.saving);
        assert!(doc.code.read_only, "accepted save freezes the buffer");
        s.editor.apply_reply(save_reply(&s, Ok(file("edited"))));
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(!doc.saving);
        assert!(!doc.code.read_only, "reply restores editability");
        assert!(!doc.dirty(), "mark_saved ran on the Ok reply");
    }

    #[test]
    fn save_failure_restores_read_only_and_keeps_buffer() {
        let mut s = checkout_state();
        dirty_editable_doc(&mut s);
        enqueue_save(&mut s);
        assert!(s.editor.doc.as_ref().unwrap().code.read_only);
        s.editor
            .apply_reply(save_reply(&s, Err("disk full".into())));
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(!doc.saving);
        assert!(!doc.code.read_only);
        assert_eq!(doc.save_error.as_deref(), Some("disk full"));
        assert!(doc.dirty(), "the draft survives a failed save");
        assert_eq!(doc.code.edit.text, "edited");
    }

    #[test]
    fn save_over_cap_never_freezes_or_enqueues() {
        let mut s = checkout_state();
        let mut doc = OpenDoc::new("a.txt", &file("orig"));
        doc.code.edit.text = "x".repeat(crate::editor::draft::TEXT_CAP + 1);
        s.editor.doc = Some(doc);
        enqueue_save(&mut s);
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(!doc.saving);
        assert!(!doc.code.read_only, "validation failure never froze");
        assert!(doc.save_error.as_ref().unwrap().contains("1 MiB"));
        assert!(s.editor.save_resume.is_none());
    }

    #[test]
    fn save_queue_refusal_leaves_read_only_untouched() {
        let mut s = checkout_state();
        dirty_editable_doc(&mut s);
        // No project_id → enqueue refuses before any freeze.
        s.editor.project_id = None;
        enqueue_save(&mut s);
        let doc = s.editor.doc.as_ref().unwrap();
        assert!(!doc.saving);
        assert!(!doc.code.read_only);
    }

    // -- ACP cancel lifecycle --------------------------------------------------

    #[test]
    fn checkout_switch_cancels_in_flight_acp() {
        let mut s = checkout_state();
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        select_checkout(&mut s, "p1".into(), PathBuf::from("/tmp/co2"));
        assert_eq!(s.editor.acp.generating_for, None);
        assert_ne!(
            s.editor.checkout_key.as_deref(),
            Some(checkout_key("p1", &PathBuf::from("/tmp/co")).as_str())
        );
    }

    #[test]
    fn project_switch_cancels_in_flight_acp() {
        let mut s = checkout_state();
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        select_project(&mut s, "p2".into());
        assert_eq!(s.editor.acp.generating_for, None);
    }

    #[test]
    fn leaving_changes_section_cancels_acp_but_entering_does_not() {
        let mut s = checkout_state();
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        s.editor.section = Section::Changes;
        // Arriving at another section FROM Changes cancels.
        set_section(&mut s, Section::History);
        assert_eq!(s.editor.acp.generating_for, None);
        // Entering Changes itself does not cancel.
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        s.editor.section = Section::Files;
        set_section(&mut s, Section::Changes);
        assert_eq!(
            s.editor.acp.generating_for.as_deref(),
            Some("p1\u{1f}/tmp/co")
        );
    }

    #[test]
    fn leaving_editor_view_cancels_acp() {
        let mut s = checkout_state();
        s.view = View::Editor;
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        let mut tx = Tx::default();
        navigate(&mut s, PendingNav::SetView(View::Terminal), &mut tx);
        assert_eq!(s.view, View::Terminal);
        assert_eq!(s.editor.acp.generating_for, None);
    }

    #[test]
    fn window_close_apply_cancels_acp() {
        let mut s = checkout_state();
        s.editor.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        let mut tx = Tx::default();
        // Clean draft → navigate applies immediately.
        navigate(&mut s, PendingNav::CloseWindow, &mut tx);
        assert_eq!(s.editor.acp.generating_for, None);
    }

    // -- sidebar → editor identity -------------------------------------------

    fn project_view(id: &str, name: &str, root: &str) -> app_core::projects::ProjectView {
        app_core::projects::ProjectView {
            project: app_core::store::Project {
                id: id.into(),
                name: name.into(),
                root: root.into(),
                created_at: 0,
            },
            missing: false,
        }
    }

    /// Two registered projects, as the Projects reply leaves them.
    fn registered_state() -> State {
        let mut s = state();
        s.editor.projects = vec![
            project_view("p1", "web", "/work/a/web"),
            project_view("p2", "api", "/work/b/api"),
        ];
        s
    }

    #[test]
    fn select_checkout_commits_real_workspace_key() {
        let mut s = registered_state();
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: "p1".into(),
                checkout: PathBuf::from("/work/a/web"),
            },
            &mut tx,
        );
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/a/web"))
        );
        // The sidebar highlight id is the desktop checkoutKey form — the
        // exact string the project-root row computes for this project.
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        // The project switch kicked its own worktree read: the picker's
        // `worktrees` AND the sidebar's per-project entry both load.
        assert!(s.editor.worktrees_loading);
        assert!(s.editor.sidebar_worktrees["p1"].loading);
    }

    #[test]
    fn select_checkout_same_project_keeps_worktree_list() {
        let mut s = registered_state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/work/a/web")));
        s.editor.worktrees = app_core::worktrees::WorktreeList {
            repo: true,
            worktrees: vec![app_core::worktrees::WorktreeInfo {
                name: "wt".into(),
                path: "/work/a/wt".into(),
                branch: None,
                head: String::new(),
                is_root: false,
                missing: false,
                dirty: None,
                changed: None,
            }],
        };
        let mut tx = Tx::default();
        // Root → linked worktree inside the SAME project: no worktree reload.
        navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: "p1".into(),
                checkout: PathBuf::from("/work/a/wt"),
            },
            &mut tx,
        );
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/a/wt"))
        );
        assert_eq!(
            s.editor.worktrees.worktrees.len(),
            1,
            "same-project switch keeps the loaded list"
        );
        assert!(!s.editor.worktrees_loading);
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/wt").as_str())
        );
    }

    #[test]
    fn set_view_editor_activates_committed_sidebar_checkout() {
        let mut s = registered_state();
        s.active_workspace = Some(crate::data::checkout_key("p2", "/work/b/api"));
        let mut tx = Tx::default();
        navigate(&mut s, PendingNav::SetView(View::Editor), &mut tx);
        assert_eq!(s.view, View::Editor);
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/b/api"))
        );
        // The activated checkout re-committed the same workspace key.
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
        );
    }

    #[test]
    fn set_view_editor_ignores_unregistered_and_mock_workspaces() {
        for bogus in [
            // Unknown/vanished project id.
            crate::data::checkout_key("ghost", "/nowhere"),
            // A mock session project id must never fabricate identity.
            crate::data::checkout_key("proj-goko", "/home/user/Project/goko"),
            // The Unassigned pseudo-workspace carries no checkout.
            crate::data::UNASSIGNED_WORKSPACE.to_string(),
        ] {
            let mut s = registered_state();
            s.active_workspace = Some(bogus.clone());
            let mut tx = Tx::default();
            navigate(&mut s, PendingNav::SetView(View::Editor), &mut tx);
            assert_eq!(s.view, View::Editor, "{bogus}: view still switches");
            assert!(
                s.editor.project_id.is_none(),
                "{bogus}: must not select a project"
            );
            assert!(
                s.editor.checkout.is_none(),
                "{bogus}: must not select a checkout"
            );
        }
    }

    #[test]
    fn sidebar_select_guard_cancel_restores_identity_and_draft() {
        let mut s = registered_state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/work/a/web")));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        // A sidebar worktree-row press while dirty: the guard holds and
        // NOTHING commits — project, checkout, workspace key and draft.
        navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: "p2".into(),
                checkout: PathBuf::from("/work/b/api"),
            },
            &mut tx,
        );
        assert!(s.editor.guard.is_some());
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        resolve_guard(&mut s, GuardAction::Cancel, &mut tx);
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert!(s.editor.doc.as_ref().unwrap().dirty());
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str()),
            "Cancel restores the previous visible workspace identity"
        );
    }

    #[test]
    fn sidebar_select_guard_discard_commits_new_checkout() {
        let mut s = registered_state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/work/a/web")));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: "p2".into(),
                checkout: PathBuf::from("/work/b/api"),
            },
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Discard, &mut tx);
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/b/api"))
        );
        assert!(s.editor.doc.is_none(), "discard dropped the draft");
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
        );
    }

    #[test]
    fn set_view_editor_guard_discard_applies_sidebar_sync() {
        let mut s = registered_state();
        s.active_workspace = Some(crate::data::checkout_key("p2", "/work/b/api"));
        dirty_doc(&mut s);
        let mut tx = Tx::default();
        navigate(&mut s, PendingNav::SetView(View::Editor), &mut tx);
        // The view did NOT flip under the guard, and the sidebar checkout
        // did not activate early either.
        assert!(s.editor.guard.is_some());
        assert_eq!(s.view, View::Terminal);
        assert!(s.editor.project_id.is_none());
        resolve_guard(&mut s, GuardAction::Discard, &mut tx);
        assert_eq!(s.view, View::Editor);
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/b/api"))
        );
    }

    #[test]
    fn project_switch_clears_committed_workspace() {
        let mut s = registered_state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/work/a/web")));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        // The editor's project picker path: SelectProject drops the foreign
        // checkout, so no workspace may stay highlighted.
        select_project(&mut s, "p2".into());
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        assert!(s.editor.checkout.is_none());
        assert_eq!(s.active_workspace, None);
        // And the new project's worktree read went through the shared
        // per-project lane the sidebar reads too.
        assert!(s.editor.sidebar_worktrees["p2"].loading);
    }

    #[test]
    fn sidebar_select_guard_save_resumes_after_landed_write() {
        let mut s = registered_state();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/work/a/web")));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        dirty_editable_doc(&mut s);
        let mut tx = Tx::default();
        navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: "p2".into(),
                checkout: PathBuf::from("/work/b/api"),
            },
            &mut tx,
        );
        resolve_guard(&mut s, GuardAction::Save, &mut tx);
        // Identity untouched while the write is in flight; the pending nav
        // rides save_resume and replays through `pending_apply` on success.
        assert_eq!(s.editor.project_id.as_deref(), Some("p1"));
        assert!(matches!(
            s.editor.save_resume.as_ref().unwrap().pending,
            PendingNav::SelectCheckout { .. }
        ));
        s.editor.apply_reply(save_reply(&s, Ok(file("edited"))));
        assert!(matches!(
            s.editor.pending_apply,
            Some(PendingNav::SelectCheckout { .. })
        ));
        // `poll` routes pending_apply back through navigate → apply.
        let pending = s.editor.pending_apply.take().unwrap();
        navigate(&mut s, pending, &mut tx);
        assert_eq!(s.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new("/work/b/api"))
        );
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
        );
    }
}

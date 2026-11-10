//! Worker-reply application for the editor backend: one `apply_reply` match
//! routes every typed reply into the owning section state, plus the write
//! follow-ups and single-document install path. Lives in a child module of
//! `backend` so it can reuse the parent's private helpers (`kind_key`) while
//! keeping `backend.rs` under the file-size budget.

use std::path::{Path, PathBuf};

use app_core::git_view::{GitViewDiff, GitViewFile};

use super::{Reply, ReplyData, Req, kind_key};
use crate::editor::state::{EditorState, GoToStage, Notice, Patch, SidebarWorktrees};

impl EditorState {
    /// Apply one worker reply. Stale results — a different checkout than the
    /// one now selected, or a superseded file-load token — are dropped, never
    /// written into live section state.
    pub fn apply_reply(&mut self, reply: Reply) {
        if self.backend.overloaded {
            self.backend.overloaded = false;
            self.notice = Some(crate::editor::state::Notice {
                text: "the editor queue is full; some refreshes were dropped".into(),
                stale: true,
            });
        }
        let current = self.checkout_key.clone();
        let scoped = reply.checkout_key.is_none() || reply.checkout_key == current;
        if !scoped {
            return; // stale checkout: drop entirely
        }
        // Same-checkout staleness: a newer read of the same kind supersedes
        // this reply even though the checkout still matches. Writes carry
        // generation 0 and always apply — a dropped save/commit reply would strand
        // its busy flag.
        if reply.generation != 0 {
            let kind = kind_key(&reply.req);
            if self.req_gens.get(kind).copied() != Some(reply.generation) {
                return;
            }
        }
        match (&reply.req, reply.result) {
            (Req::Projects, Ok(ReplyData::Projects(projects))) => {
                self.projects = projects;
                self.projects_loading = false;
                self.projects_error = None;
                // Sidebar worktree state is keyed by project id: drop
                // entries for projects that vanished and (re)load every
                // project with no list yet or a stale error.
                let ids: Vec<String> = self.projects.iter().map(|p| p.project.id.clone()).collect();
                self.sidebar_worktrees
                    .retain(|pid, _| ids.iter().any(|id| id == pid));
                for pid in &ids {
                    let needs = self
                        .sidebar_worktrees
                        .get(pid)
                        .is_none_or(|e| e.list.is_none() || e.error.is_some());
                    if needs {
                        self.request_worktrees(pid);
                    }
                }
                // The selected project disappeared: keep the identity (the
                // pickers show it unselected) but refresh so section reads
                // surface the honest not-found error, not stale data.
                if self
                    .project_id
                    .as_ref()
                    .is_some_and(|pid| !ids.contains(pid))
                {
                    self.refresh_active();
                }
            }
            (Req::Projects, Err(e)) => {
                self.projects_loading = false;
                self.projects_error = Some(e);
            }
            (Req::AddProject(_), Ok(ReplyData::Added(p))) => {
                if !self.projects.iter().any(|x| x.project.id == p.project.id) {
                    self.projects.push(p);
                }
                self.refresh_projects();
            }
            (Req::AddProject(_), Err(e)) => self.notice = Some(err_notice(&e)),
            (Req::Worktrees(pid), Ok(ReplyData::Worktrees(list))) => {
                // Replies are keyed by the request's project id: only the
                // selected project's list feeds the checkout picker.
                if self.project_id.as_deref() == Some(pid.as_str()) {
                    self.worktrees = list.clone();
                    self.worktrees_loading = false;
                    self.worktrees_error = None;
                }
                self.sidebar_worktrees.insert(
                    pid.clone(),
                    SidebarWorktrees {
                        list: Some(list),
                        loading: false,
                        error: None,
                    },
                );
                // A fresh list that dropped the selected checkout means the
                // worktree was pruned or moved: refresh so section reads
                // fail honestly now instead of serving stale paths.
                if self.project_id.as_deref() == Some(pid.as_str()) && !self.checkout_still_valid()
                {
                    self.refresh_active();
                }
            }
            (Req::Worktrees(pid), Err(e)) => {
                if self.project_id.as_deref() == Some(pid.as_str()) {
                    self.worktrees_loading = false;
                    self.worktrees_error = Some(e.clone());
                }
                let entry = self.sidebar_worktrees.entry(pid.clone()).or_default();
                entry.loading = false;
                entry.error = Some(e);
            }
            // -- New-worktree form (project-level jobs, like `Worktrees`) --
            (Req::WorktreeBranches { project_id }, Ok(ReplyData::WorktreeBranches(refs))) => {
                if let Some(f) = self
                    .worktree_form
                    .as_mut()
                    .filter(|f| f.project_id == *project_id)
                {
                    f.branches = Some(refs);
                    f.branches_loading = false;
                    f.branches_error = None;
                    crate::editor::worktree_form::branches_loaded(f);
                }
            }
            (Req::WorktreeBranches { project_id }, Err(e)) => {
                if let Some(f) = self
                    .worktree_form
                    .as_mut()
                    .filter(|f| f.project_id == *project_id)
                {
                    f.branches_loading = false;
                    f.branches_error = Some(e);
                }
            }
            (
                Req::EnvCandidates {
                    project_id,
                    source_path,
                },
                Ok(ReplyData::EnvCandidates(files)),
            ) => {
                if let Some(f) = self
                    .worktree_form
                    .as_mut()
                    .filter(|f| f.project_id == *project_id)
                {
                    // Only the preview that asked for THIS source applies —
                    // a stale reply from a previous source would mislead.
                    if f.env_requested.as_deref() == Some(source_path.as_str()) {
                        f.env_files = Some(files);
                        f.env_loading = false;
                        f.env_error = None;
                    }
                }
            }
            (
                Req::EnvCandidates {
                    project_id,
                    source_path,
                },
                Err(e),
            ) => {
                if let Some(f) = self
                    .worktree_form
                    .as_mut()
                    .filter(|f| f.project_id == *project_id)
                {
                    // Same latch as the Ok arm: a stale source's error must
                    // never clobber the preview the current source owns.
                    // Clearing `env_requested` unlatches the failure so a
                    // deliberate retry (toggle/Retry) can re-issue.
                    if f.env_requested.as_deref() == Some(source_path.as_str()) {
                        f.env_loading = false;
                        f.env_error = Some(e);
                        f.env_requested = None;
                    }
                }
            }
            (
                Req::WorktreeCreate {
                    project_id,
                    env_source,
                    ..
                },
                Ok(ReplyData::WorktreeCreated(created)),
            ) => {
                // The new checkout joins the project's list (same refresh
                // the legacy `Req::CreateWorktree` reply triggers).
                self.request_worktrees(project_id);
                if let Some(env) = &created.env {
                    let (report, error) = match env {
                        Ok(report) => (Some(report.clone()), None),
                        Err(e) => (None, Some(e.clone())),
                    };
                    self.worktree_env.insert(
                        project_id.clone(),
                        crate::editor::worktree_form::EnvNotice {
                            source_path: env_source.clone().unwrap_or_default(),
                            target_path: created.info.path.clone(),
                            report,
                            error,
                            retrying: false,
                        },
                    );
                }
                if self
                    .worktree_form
                    .as_ref()
                    .is_some_and(|f| f.project_id == *project_id)
                {
                    self.worktree_form = None;
                }
                // The desktop's `activateWorkspace(checkoutKey(project.id,
                // worktree.path))`: navigate into the created checkout
                // through `pending_apply`, so the draft guard still applies
                // like any other checkout switch. A queued navigation wins
                // — never clobber it.
                if self.pending_apply.is_none()
                    && self.guard.is_none()
                    && self.save_resume.is_none()
                {
                    self.pending_apply =
                        Some(crate::editor::navigation::PendingNav::SelectCheckout {
                            project_id: project_id.clone(),
                            checkout: PathBuf::from(&created.info.path),
                        });
                }
            }
            (Req::WorktreeCreate { project_id, .. }, Err(e)) => {
                if let Some(f) = self
                    .worktree_form
                    .as_mut()
                    .filter(|f| f.project_id == *project_id)
                {
                    f.submitting = false;
                    f.error = Some(e);
                } else {
                    self.notice = Some(err_notice(&e));
                }
            }
            (Req::EnvCopy { project_id, .. }, Ok(ReplyData::EnvCopy(report))) => {
                if let Some(notice) = self.worktree_env.get_mut(project_id) {
                    notice.report = Some(report);
                    notice.error = None;
                    notice.retrying = false;
                }
            }
            (Req::EnvCopy { project_id, .. }, Err(e)) => {
                if let Some(notice) = self.worktree_env.get_mut(project_id) {
                    notice.report = None;
                    notice.error = Some(e);
                    notice.retrying = false;
                } else {
                    self.notice = Some(err_notice(&e));
                }
            }
            // `load` never fails: corrupt JSON falls back per field, exactly
            // like the desktop's readLayout. Other keys have no consumer yet.
            (Req::GetSetting { key }, Ok(ReplyData::Setting(value)))
                if key == crate::editor::history_layout::HISTORY_LAYOUT_KEY =>
            {
                self.history.layout =
                    crate::editor::history_layout::HistoryLayout::load(value.as_deref());
                // One-time seed of TableState widths — after this,
                // `column_width == None` means auto/reset only, so the graph
                // override survives restart and double-click reset sticks.
                crate::editor::history::seed_columns_from_layout(&mut self.history);
            }
            (Req::GetSetting { key }, Ok(ReplyData::Setting(value)))
                if key == crate::editor::lsp::SERVERS_KEY =>
            {
                self.lsp.apply_config(value.as_deref());
            }
            // Indent override restores before/after docs open: mirror it
            // into whichever code surfaces exist right now.
            (Req::GetSetting { key }, Ok(ReplyData::Setting(value)))
                if key == crate::editor::indent::INDENT_KEY =>
            {
                let choice =
                    crate::editor::indent::UnitChoice::parse(value.as_deref().unwrap_or("auto"));
                self.indent_choice = choice;
                if let Some(doc) = self.doc.as_mut() {
                    doc.code.set_indent_choice(choice);
                }
                self.scratch_code.set_indent_choice(choice);
            }
            // The inline-blame toggle restores on the same lane: an absent
            // value means the default (on).
            (Req::GetSetting { key }, Ok(ReplyData::Setting(value)))
                if key == crate::editor::blame::BLAME_KEY =>
            {
                let enabled = value.as_deref().unwrap_or("1") != "0";
                self.apply_blame_enabled(enabled);
                crate::editor::blame::sync_visibility(self);
                if enabled {
                    crate::editor::blame::request(self);
                }
            }
            (Req::GetSetting { key }, Err(e)) if key == crate::editor::lsp::SERVERS_KEY => {
                // Un-pend + bounded retry live in lsp.rs; presets keep
                // serving until a read succeeds.
                self.lsp.config_get_failed(e);
            }
            (Req::SetSetting { key, .. }, Err(e)) if key == crate::editor::lsp::SERVERS_KEY => {
                self.lsp.persist_failed(e);
            }
            (Req::SetSetting { .. }, _) => {
                // Ok or Err alike: the write attempt is done; a still-dirty
                // flag re-arms on the next poll, bounded by user activity.
                self.history.layout_save_pending = false;
            }
            (Req::Changes, Ok(ReplyData::Changes(data))) => {
                self.changes.data = Some(data);
                self.changes.checkout = current.clone();
                self.changes.loading = false;
                self.changes.error = None;
            }
            (Req::Changes, Err(e)) => {
                self.changes.loading = false;
                if self.changes.data.is_none() {
                    self.changes.error = Some(e);
                } else {
                    self.notice = Some(stale_notice(&e));
                }
            }
            (
                Req::Diff {
                    layer,
                    path,
                    commit,
                },
                Ok(ReplyData::Diff(diff)),
            ) => {
                let key = current.clone().unwrap_or_default();
                let patch = patch_from_diff(diff, &key);
                if let Some(commit) = commit {
                    // Commit detail (history pane): only the selected commit.
                    if self.history.selected.as_deref() == Some(commit.as_str()) {
                        self.history
                            .diff
                            .set_files(rune_kit::diff::parse(&patch.raw));
                        self.history.detail = Some(patch);
                        self.history.detail_loading = false;
                    }
                } else if self.changes.layer.as_deref() == Some(layer.as_str())
                    && self.changes.patch_path == *path
                {
                    self.changes
                        .diff
                        .set_files(rune_kit::diff::parse(&patch.raw));
                    self.changes.patch = Some(patch);
                    self.changes.patch_loading = false;
                }
            }
            (
                Req::Diff {
                    commit: Some(_), ..
                },
                Err(e),
            ) => {
                // Commit-detail diff (history pane): release its spinner and
                // surface the failure — the changes patch lane is untouched.
                self.history.detail_loading = false;
                self.notice = Some(err_notice(&e));
            }
            (Req::Diff { .. }, Err(e)) => {
                self.changes.patch_loading = false;
                self.changes.error = Some(e);
            }
            (Req::History { skip, branch }, Ok(ReplyData::History(page))) => {
                let matches_filter = self.history.branch == *branch;
                if matches_filter {
                    if *skip == 0 {
                        self.history.commits = page.commits;
                    } else {
                        self.history.commits.extend(page.commits);
                    }
                    self.history.next_skip = page.next_skip;
                    self.history.checkout = current.clone();
                    self.history.loading = false;
                    self.history.loading_more = false;
                    self.history.error = None;
                }
            }
            (Req::History { .. }, Err(e)) => {
                self.history.loading = false;
                self.history.loading_more = false;
                if self.history.commits.is_empty() {
                    self.history.error = Some(e);
                } else {
                    self.notice = Some(stale_notice(&e));
                }
            }
            (Req::Branches, Ok(ReplyData::Branches(branches))) => {
                self.history.branches = branches;
            }
            (Req::FileList, Ok(ReplyData::FileList(paths))) => {
                self.files.paths = paths;
                self.files.loading = false;
                self.files.error = None;
                self.rebuild_file_model();
                // A same-count path swap leaves `matched_len`/checkout
                // untouched, so `quick_open::poll` can't see it — an open
                // palette recomputes here instead of offering removed
                // paths and hiding new ones.
                if self.quick_open.open {
                    crate::editor::quick_open::refresh_matches(self);
                }
            }
            (Req::FileList, Err(e)) => {
                self.files.loading = false;
                if self.files.paths.is_empty() {
                    self.files.error = Some(e);
                } else {
                    self.notice = Some(stale_notice(&e));
                }
            }
            (Req::Search { query }, Ok(ReplyData::Search(result)))
                if self.files.search.query == *query =>
            {
                self.files.search.results = Some(result);
                self.files.search.searching = false;
                self.files.search.error = None;
            }
            (Req::GoToSearch { query }, Ok(ReplyData::Search(result))) => {
                crate::editor::navigation::go_to_search_reply(self, query.clone(), result);
            }
            (Req::GoToSearch { .. }, Err(e)) => {
                self.go_to = None;
                self.notice = Some(err_notice(&e));
            }
            (Req::ConfigFiles { .. }, Ok(ReplyData::ConfigAliases(aliases))) => {
                crate::editor::navigation::go_to_config_reply(self, aliases);
            }
            (Req::ConfigFiles { .. }, Err(e)) => {
                // Only an in-flight import lookup owns this lane; a name
                // search pending meanwhile must survive the error.
                if self
                    .go_to
                    .as_ref()
                    .is_some_and(|p| matches!(p.stage, GoToStage::AwaitConfigs { .. }))
                {
                    self.go_to = None;
                }
                self.notice = Some(err_notice(&e));
            }
            (Req::Search { .. }, Err(e)) => {
                self.files.search.searching = false;
                self.files.search.error = Some(e);
            }
            (Req::File { path, token, line }, Ok(ReplyData::File(file)))
                if self.doc_load == Some(*token) =>
            {
                self.doc_load = None;
                self.install_doc(path.clone(), file, *line);
            }
            (Req::Highlight { path, .. }, Ok(ReplyData::Highlight(result))) => {
                crate::editor::highlight::apply(self, path, result);
            }
            (Req::Highlight { path, .. }, Err(_)) => {
                crate::editor::highlight::apply_error(self, path);
            }
            (Req::Blame { path }, Ok(ReplyData::Blame(view))) => {
                crate::editor::blame::apply(self, path, view);
            }
            (Req::Blame { path }, Err(e)) => {
                crate::editor::blame::apply_error(self, path, &e);
            }
            (Req::File { .. }, Err(e)) => {
                self.doc_load = None;
                self.notice = Some(err_notice(&e));
            }
            (
                Req::ReloadFile {
                    path,
                    token,
                    revision,
                },
                Ok(ReplyData::File(file)),
            ) => {
                self.apply_reload(path, *token, *revision, file);
            }
            (Req::ReloadFile { path, .. }, Err(e)) => {
                // The request is consumed either way; the buffer, its
                // save_error and undo history are untouched — only the
                // failure surfaces.
                self.doc_reload = None;
                self.notice = Some(Notice {
                    text: format!("cannot reload '{path}': {e}"),
                    stale: false,
                });
            }
            (Req::Stashes, Ok(ReplyData::Stashes(list))) => {
                self.stashes.stashes = list;
                self.stashes.checkout = current.clone();
                self.stashes.shared_repo = true;
                self.stashes.loading = false;
                self.stashes.error = None;
            }
            (Req::Stashes, Err(e)) => {
                self.stashes.loading = false;
                if self.stashes.stashes.is_empty() {
                    self.stashes.error = Some(e);
                } else {
                    self.notice = Some(stale_notice(&e));
                }
            }
            (Req::StashDiff { stash }, Ok(ReplyData::StashDiff(diff)))
                if self.stashes.selected.as_deref() == Some(stash.as_str()) =>
            {
                let patch = patch_from_diff(diff, &current.clone().unwrap_or_default());
                self.stashes
                    .diff
                    .set_files(rune_kit::diff::parse(&patch.raw));
                self.stashes.detail = Some(patch);
                self.stashes.detail_loading = false;
            }
            (Req::StashDiff { .. }, Err(e)) => {
                self.stashes.detail_loading = false;
                self.notice = Some(err_notice(&e));
            }
            (
                Req::ExternalOpen { project_id, .. },
                Ok(ReplyData::ExternalOpen {
                    checkout,
                    path,
                    line,
                }),
            ) => {
                self.apply_external_open(project_id, checkout, path, line);
            }
            (Req::ExternalOpen { .. }, Err(e)) => {
                self.notice = Some(err_notice(&e));
            }
            (Req::SaveFile { path, .. }, Ok(ReplyData::Saved(file))) => {
                self.finish_save(path.clone(), Ok(file));
            }
            (Req::SaveFile { path, .. }, Err(e)) => {
                self.finish_save(path.clone(), Err(e));
            }
            (Req::Stage { .. } | Req::Unstage { .. } | Req::Discard { .. }, Ok(ReplyData::Ok)) => {
                self.changes.busy = None;
                self.changes.error = None;
                self.after_write("changes");
            }
            (Req::Stage { .. } | Req::Unstage { .. } | Req::Discard { .. }, Err(e)) => {
                self.changes.busy = None;
                self.changes.error = Some(e.clone());
                self.notice = Some(err_notice(&e));
            }
            (Req::Commit { .. }, Ok(ReplyData::Commit(result))) => {
                self.commit.committing = false;
                self.commit.message = rune::edit::TextEdit::new("");
                self.after_write("commit");
                let branch = result.branch.as_deref().unwrap_or("HEAD");
                self.notice = Some(crate::editor::state::Notice {
                    text: format!(
                        "committed {} on {branch}",
                        &result.head[..result.head.len().min(8)]
                    ),
                    stale: false,
                });
            }
            (Req::Commit { .. }, Err(e)) => {
                self.commit.committing = false;
                self.commit.error = Some(e.clone());
                self.notice = Some(err_notice(&e));
            }
            (Req::Push, Ok(ReplyData::Push(result))) => {
                self.commit.pushing = false;
                self.after_write("push");
                if !result.summary.is_empty() {
                    self.notice = Some(crate::editor::state::Notice {
                        text: result.summary,
                        stale: false,
                    });
                }
            }
            (Req::Push, Err(e)) => {
                self.commit.pushing = false;
                self.commit.error = Some(e.clone());
                self.notice = Some(err_notice(&e));
            }
            _ => {}
        }
    }

    fn finish_save(&mut self, path: String, result: Result<GitViewFile, String>) {
        let resume = self.save_resume.take();
        if self.doc.as_ref().is_some_and(|d| d.path == path) {
            let doc = self.doc.as_mut().unwrap();
            doc.saving = false;
            match &result {
                Ok(file) => {
                    doc.mark_saved(file);
                    doc.save_error = None;
                }
                Err(e) => {
                    doc.save_error = Some(e.clone());
                }
            }
            // The save flight is over on every outcome — restore the doc's
            // own editability. The freeze only changed input handling, so
            // the TextEdit model and its undo history were never touched.
            doc.code.read_only = !doc.kind.editable() || doc.truncated;
        }
        self.sync_quit_dirty();
        if result.is_ok() {
            // A landed save lets the LSP mirror emit its didSave — only
            // when the saved path is still the mirrored document.
            self.lsp_note_saved(&path);
            // The saved file is what Git blames, so the annotations are
            // re-read once the draft matches the disk again.
            crate::editor::blame::sync_visibility(self);
            crate::editor::blame::request(self);
        }
        match result {
            Ok(_) => {
                // A successful save resumes the navigation that was waiting.
                if let Some(guard) = resume {
                    self.pending_apply = Some(guard.pending);
                }
            }
            Err(_) => {
                // Failed save retains the draft and cancels the navigation.
            }
        }
    }

    /// A validated external open request: queue it as ONE guarded pending
    /// carrying project+checkout+path, so the draft guard treats the
    /// checkout switch and the doc open atomically — consumed exactly once,
    /// never split into an orphaned open against the wrong checkout.
    ///
    /// The pending slot is single: when another navigation is already
    /// queued, guarded, or waiting on a save-resume, the request is refused
    /// loudly instead of overwriting it — the earlier request always wins.
    fn apply_external_open(
        &mut self,
        project_id: &str,
        checkout: PathBuf,
        path: String,
        line: Option<u64>,
    ) {
        if self.pending_apply.is_some() || self.guard.is_some() || self.save_resume.is_some() {
            self.notice = Some(crate::editor::state::Notice {
                text: "another navigation is already pending; the external open was ignored".into(),
                stale: true,
            });
            return;
        }
        self.pending_apply = Some(crate::editor::navigation::PendingNav::ExternalOpen {
            project_id: project_id.to_string(),
            checkout,
            path,
            line,
        });
    }

    /// Refresh the checkout data a completed write affects, keyed to the
    /// checkout the write ran on — if the user has since switched, the
    /// replies drop on the staleness check instead of clobbering the new
    /// selection.
    fn after_write(&mut self, what: &str) {
        let Some(key) = self.checkout_key.clone() else {
            return;
        };
        let Some(checkout) = self.checkout.clone() else {
            return;
        };
        match what {
            "commit" => {
                self.enqueue_read(checkout.clone(), key.clone(), Req::Changes);
                self.enqueue_read(
                    checkout.clone(),
                    key.clone(),
                    Req::History {
                        skip: 0,
                        branch: self.history.branch.clone(),
                    },
                );
                self.enqueue_read(checkout, key, Req::Branches);
            }
            "push" => {
                self.enqueue_read(checkout.clone(), key.clone(), Req::Changes);
            }
            _ => {
                self.enqueue_read(checkout.clone(), key.clone(), Req::Changes);
                self.enqueue_patch_refresh(checkout, key);
            }
        }
    }

    /// `true` while the selected checkout is still selectable: its project
    /// is registered and the checkout is the project root or a listed,
    /// non-missing worktree. An empty project list mid-load counts as
    /// "unknown", not invalid, so startup replies never trigger a spurious
    /// refresh.
    fn checkout_still_valid(&self) -> bool {
        let (Some(pid), Some(checkout)) = (self.project_id.as_deref(), self.checkout.as_ref())
        else {
            return true;
        };
        let Some(project) = self.projects.iter().find(|p| p.project.id == pid) else {
            return self.projects.is_empty() && self.projects_loading;
        };
        if checkout.as_path() == Path::new(&project.project.root) {
            return true;
        }
        self.worktrees
            .worktrees
            .iter()
            .any(|wt| !wt.missing && Path::new(&wt.path) == checkout.as_path())
    }

    fn enqueue_patch_refresh(&mut self, checkout: PathBuf, key: String) {
        if let (Some(layer), Some(path)) =
            (self.changes.layer.clone(), self.changes.patch_path.clone())
        {
            self.enqueue_read(
                checkout,
                key,
                Req::Diff {
                    layer,
                    path: Some(path),
                    commit: None,
                },
            );
        }
    }
}

/// Git marks binary diffs in the patch text itself; surface that as a flag
/// the patch pane can banner instead of trying to render it as text.
fn patch_from_diff(diff: GitViewDiff, checkout: &str) -> Patch {
    let binary = diff.patch.contains("Binary files") || diff.patch.contains("GIT binary patch");
    Patch {
        label: diff.label,
        raw: diff.patch,
        truncated: diff.truncated,
        binary,
        files: diff.files,
        detail: diff.detail,
        checkout: checkout.to_string(),
    }
}

fn err_notice(e: &str) -> crate::editor::state::Notice {
    crate::editor::state::Notice {
        text: e.to_string(),
        stale: false,
    }
}

fn stale_notice(e: &str) -> crate::editor::state::Notice {
    crate::editor::state::Notice {
        text: format!("refresh failed; showing cached data — {e}"),
        stale: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;
    use crate::editor::state::Section;
    use app_core::projects::ProjectView;
    use app_core::store::Project;
    use app_core::worktrees::{WorktreeInfo, WorktreeList};

    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/tmp/co"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/tmp/co"),
        ));
        s
    }

    fn pv(id: &str, name: &str, root: &str) -> ProjectView {
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

    fn wt_list(worktrees: Vec<WorktreeInfo>) -> WorktreeList {
        WorktreeList {
            repo: true,
            worktrees,
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

    // -- per-project project/worktree replies -------------------------------

    #[test]
    fn projects_reply_seeds_sidebar_worktrees_per_project() {
        let mut s = scene();
        s.editor.apply_reply(reply(
            Req::Projects,
            Ok(ReplyData::Projects(vec![
                pv("p1", "web", "/work/a/web"),
                pv("p2", "api", "/work/b/api"),
            ])),
        ));
        assert_eq!(s.editor.projects.len(), 2);
        // Every registered project got its own in-flight sidebar read —
        // keyed by id, so a reply can only land on the project it asked for.
        assert!(s.editor.sidebar_worktrees["p1"].loading);
        assert!(s.editor.sidebar_worktrees["p2"].loading);
        assert_eq!(s.editor.sidebar_worktrees.len(), 2);
    }

    #[test]
    fn worktrees_reply_for_unselected_project_stays_sidebar_only() {
        let mut s = scene();
        s.editor.projects = vec![
            pv("p1", "web", "/work/a/web"),
            pv("p2", "api", "/work/b/api"),
        ];
        s.editor.apply_reply(reply(
            Req::Worktrees("p1".into()),
            Ok(ReplyData::Worktrees(wt_list(vec![
                wt("web", "/work/a/web", true),
                wt("feat", "/work/a/feat", false),
            ]))),
        ));
        assert_eq!(s.editor.worktrees.worktrees.len(), 2);
        // p2's reply must NOT overwrite the selected project's picker list.
        s.editor.apply_reply(reply(
            Req::Worktrees("p2".into()),
            Ok(ReplyData::Worktrees(wt_list(vec![wt(
                "api",
                "/work/b/api",
                true,
            )]))),
        ));
        assert_eq!(
            s.editor.worktrees.worktrees.len(),
            2,
            "unselected project's list never lands in the picker"
        );
        assert_eq!(
            s.editor.worktrees.worktrees[0].name, "web",
            "picker still shows p1's list"
        );
        assert_eq!(
            s.editor.sidebar_worktrees["p2"]
                .list
                .as_ref()
                .unwrap()
                .worktrees
                .len(),
            1,
            "p2's list lives in its own sidebar entry"
        );
    }

    #[test]
    fn worktrees_error_lands_on_owning_project_only() {
        let mut s = scene();
        s.editor.projects = vec![
            pv("p1", "web", "/work/a/web"),
            pv("p2", "api", "/work/b/api"),
        ];
        s.editor.apply_reply(reply(
            Req::Worktrees("p2".into()),
            Err("git exploded".into()),
        ));
        assert!(s.editor.worktrees_error.is_none(), "picker error untouched");
        assert_eq!(
            s.editor.sidebar_worktrees["p2"].error.as_deref(),
            Some("git exploded")
        );
        assert!(!s.editor.sidebar_worktrees["p2"].loading);
    }

    #[test]
    fn vanished_selected_project_prunes_sidebar_and_refreshes() {
        let mut s = scene();
        s.editor.projects = vec![pv("p1", "web", "/work/a/web")];
        s.editor.sidebar_worktrees.insert(
            "p1".into(),
            SidebarWorktrees {
                list: Some(wt_list(vec![wt("web", "/work/a/web", true)])),
                loading: false,
                error: None,
            },
        );
        s.editor.sidebar_worktrees.insert(
            "gone".into(),
            SidebarWorktrees {
                list: None,
                loading: true,
                error: None,
            },
        );
        // p1 vanished from the registry; "gone" was never registered.
        s.editor.apply_reply(reply(
            Req::Projects,
            Ok(ReplyData::Projects(vec![pv("p2", "api", "/work/b/api")])),
        ));
        assert_eq!(s.editor.projects.len(), 1);
        assert!(
            !s.editor.sidebar_worktrees.contains_key("p1")
                && !s.editor.sidebar_worktrees.contains_key("gone"),
            "vanished projects lose their sidebar entries"
        );
        // The selected project is gone: section reads re-kicked so they
        // surface the honest not-found error (Changes is the default
        // section, so its loading flag arms).
        assert_eq!(s.editor.section, Section::Changes);
        assert!(
            s.editor.changes.loading,
            "refresh ran for the dead checkout"
        );
    }

    #[test]
    fn vanished_selected_worktree_triggers_refresh() {
        let mut s = scene();
        // /tmp/co is neither the project root nor a listed worktree.
        s.editor.projects = vec![pv("p1", "web", "/work/a/web")];
        s.editor.worktrees = wt_list(vec![wt("web", "/work/a/web", true)]);
        s.editor.apply_reply(reply(
            Req::Worktrees("p1".into()),
            Ok(ReplyData::Worktrees(wt_list(vec![wt(
                "web",
                "/work/a/web",
                true,
            )]))),
        ));
        assert!(
            s.editor.changes.loading,
            "checkout no longer selectable → honest refresh"
        );
    }

    #[test]
    fn listed_selected_worktree_does_not_refresh() {
        let mut s = scene();
        s.editor.projects = vec![pv("p1", "web", "/work/a/web")];
        s.editor.worktrees = wt_list(vec![
            wt("web", "/work/a/web", true),
            wt("co", "/tmp/co", false),
        ]);
        s.editor.apply_reply(reply(
            Req::Worktrees("p1".into()),
            Ok(ReplyData::Worktrees(wt_list(vec![
                wt("web", "/work/a/web", true),
                wt("co", "/tmp/co", false),
            ]))),
        ));
        assert!(
            !s.editor.changes.loading,
            "checkout still listed → no spurious refresh"
        );
    }

    #[test]
    fn root_checkout_stays_valid_without_worktree_listing() {
        let mut s = scene();
        // Select the project ROOT as checkout: valid even when the worktree
        // list doesn't repeat it.
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/work/a/web"),
        ));
        s.editor.projects = vec![pv("p1", "web", "/work/a/web")];
        s.editor.worktrees = wt_list(vec![wt("web", "/work/a/web", true)]);
        s.editor.apply_reply(reply(
            Req::Worktrees("p1".into()),
            Ok(ReplyData::Worktrees(wt_list(vec![wt(
                "web",
                "/work/a/web",
                true,
            )]))),
        ));
        assert!(!s.editor.changes.loading);
    }

    fn file_list_reply(paths: Vec<String>) -> Reply {
        Reply {
            req: Req::FileList,
            generation: 0,
            checkout_key: None,
            result: Ok(ReplyData::FileList(paths)),
        }
    }

    /// The reported staleness: `quick_open::poll` only compares the path
    /// count and checkout key, so a FileList refresh that swaps one path
    /// for another at the SAME count leaves an open palette offering a
    /// removed file and hiding the new one. The reply itself must refresh.
    #[test]
    fn file_list_reply_refreshes_open_palette_on_same_count_swap() {
        let mut s = scene();
        s.editor.files.paths = vec!["src/a.rs".into(), "src/b.rs".into()];
        crate::editor::quick_open::open(&mut s);
        assert_eq!(
            s.editor.quick_open.matches,
            vec!["src/a.rs", "src/b.rs"],
            "palette built from the pre-reply cache"
        );

        s.editor
            .apply_reply(file_list_reply(vec!["src/a.rs".into(), "src/c.rs".into()]));

        assert_eq!(s.editor.files.paths, vec!["src/a.rs", "src/c.rs"]);
        assert_eq!(
            s.editor.quick_open.matches,
            vec!["src/a.rs", "src/c.rs"],
            "open palette must drop the removed path and show the new one"
        );
    }

    /// The refresh is gated on the palette being open: a closed palette's
    /// stale leftovers are rebuilt by `open` anyway, so the reply must not
    /// pay a rescore over the whole path cache for nothing.
    #[test]
    fn file_list_reply_skips_recompute_while_palette_closed() {
        let mut s = scene();
        s.editor.files.paths = vec!["src/a.rs".into()];
        s.editor.quick_open.matches = vec!["stale".into()];
        s.editor.quick_open.matched_len = 99;

        s.editor
            .apply_reply(file_list_reply(vec!["src/a.rs".into(), "src/c.rs".into()]));

        assert_eq!(s.editor.files.paths, vec!["src/a.rs", "src/c.rs"]);
        assert_eq!(
            s.editor.quick_open.matches,
            vec!["stale"],
            "closed palette keeps its leftovers — open() rebuilds them"
        );
        assert_eq!(s.editor.quick_open.matched_len, 99);
    }
}

//! Blocking filesystem/Git work, kept off the UI thread.
//!
//! One worker thread owns a `Storage` connection and a bounded job queue;
//! every job carries the checkout identity it belongs to so a late reply can
//! never land on another checkout's state. Writes ride the same single lane,
//! which serializes them for free. Replies reach the UI through
//! `AppHandle::update`, which also wakes the event loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};

use app_core::error::BridgeError;
use app_core::git_view::{
    self, GitViewBranch, GitViewChanges, GitViewDiff, GitViewFile, GitViewHistory, GitViewSearch,
    GitViewStash,
};
use app_core::git_write::{self, GitCommitResult, GitPushResult};
#[cfg(not(test))]
use app_core::home;
use app_core::projects::{self, ProjectView};
use app_core::store::Storage;
use app_core::worktrees::{self, WorktreeList};
use rune::AppHandle;

use crate::State;
use crate::editor::state::{EditorState, Notice, Section};
use crate::editor::syntax;

mod apply;

/// Pending-job cap: beyond this, new reads are dropped with a notice rather
/// than queueing unboundedly behind a slow `git` call.
pub const PENDING_CAP: usize = 128;

/// Every backend operation, request and reply share this enum so the reply
/// can be matched back to exactly what was asked, scoped by `checkout_key`.
#[derive(Clone, Debug)]
pub enum Req {
    // project / checkout metadata
    Projects,
    AddProject(String),
    Worktrees(String),
    /// Settings-store metadata (project-level, no checkout): `GetSetting`
    /// returns the stored string for startup restore; `SetSetting` persists
    /// pane layout and other small UI state. No schema change — the shared
    /// `settings` table already exists.
    GetSetting {
        key: String,
    },
    SetSetting {
        key: String,
        value: String,
    },
    // reads (all take the job's checkout)
    Changes,
    /// `layer` is a git_view layer (`staged`, `unstaged`, `untracked`,
    /// `conflict`, `commit`); `commit` carries the sha for `layer == "commit"`
    /// (history detail), `path` narrows file layers.
    Diff {
        layer: String,
        path: Option<String>,
        commit: Option<String>,
    },
    History {
        skip: u64,
        branch: Option<String>,
    },
    Branches,
    FileList,
    Search {
        query: String,
    },
    /// Content search for go-to-definition name resolution — a separate
    /// request kind so it never supersedes (or is superseded by) the
    /// search pane's own queries.
    GoToSearch {
        query: String,
    },
    /// Read checkout config candidates WITHOUT installing them — ONE request
    /// carries the ordered `tsconfig`/`jsconfig` list (nearest first) and the
    /// worker returns the first `paths` aliases found, for go-to-definition.
    ConfigFiles {
        paths: Vec<String>,
    },
    /// Syntax-highlight the live draft text (pure CPU on the worker; no
    /// Git/storage access). `revision` is the `CodeState` revision the text
    /// was captured at — replies apply only while it still matches.
    Highlight {
        path: String,
        revision: u64,
        text: String,
    },
    File {
        path: String,
        token: u64,
        line: Option<u64>,
    },
    /// Explicit "reload from disk" for the open doc (the guard dialog's
    /// escape hatch after a failed save). Same `git_view::file` read as
    /// `File`, but the reply replaces the whole `OpenDoc` — baseline AND
    /// buffer — only while token/path/revision/checkout still match.
    /// `revision` is the `CodeState` revision at request time: any edit
    /// during the flight bumps it and the reply drops, preserving edits.
    ReloadFile {
        path: String,
        token: u64,
        revision: u64,
    },
    Stashes,
    StashDiff {
        stash: String,
    },
    /// External open-file/line request: validated against the registered
    /// project+checkout on the worker (no UI-thread storage reads), then
    /// applied once as checkout switch + doc open.
    ExternalOpen {
        project_id: String,
        checkout_path: String,
        path: String,
        line: Option<u64>,
    },
    // writes (serialized on the same lane)
    SaveFile {
        path: String,
        expected: String,
        text: String,
    },
    Stage {
        paths: Option<Vec<String>>,
    },
    Unstage {
        paths: Option<Vec<String>>,
    },
    Discard {
        layer: String,
        path: String,
    },
    Commit {
        message: String,
    },
    Push,
}

#[derive(Debug)]
pub enum ReplyData {
    Projects(Vec<ProjectView>),
    Added(ProjectView),
    Worktrees(WorktreeList),
    Setting(Option<String>),
    Changes(GitViewChanges),
    Diff(GitViewDiff),
    History(GitViewHistory),
    Branches(Vec<GitViewBranch>),
    FileList(Vec<String>),
    Search(GitViewSearch),
    File(GitViewFile),
    Highlight(crate::editor::syntax::HighlightResult),
    Stashes(Vec<GitViewStash>),
    StashDiff(GitViewDiff),
    ExternalOpen {
        checkout: PathBuf,
        path: String,
        line: Option<u64>,
    },
    Saved(GitViewFile),
    /// The config lookup's answer: `paths` aliases parsed worker-side from
    /// the nearest `tsconfig`/`jsconfig` candidate that defined any
    /// (empty when none did).
    ConfigAliases(Vec<crate::editor::go_to_definition::PathAlias>),
    Ok,
    Commit(GitCommitResult),
    Push(GitPushResult),
}

#[derive(Debug)]
pub struct Reply {
    /// Echo of the request, for matching replies to section state.
    pub req: Req,
    /// Read generation the job was issued under (`0` for writes).
    pub generation: u64,
    /// Checkout the job ran against (`None` for project-level calls).
    pub checkout_key: Option<String>,
    pub result: Result<ReplyData, String>,
}

#[derive(Clone, Debug)]
pub struct Job {
    /// Registered project the checkout belongs to. Every checkout-scoped job
    /// revalidates `project_id` + path through `projects::git_checkout`
    /// immediately before touching Git, so a removed project, unregistered
    /// path or swapped worktree can never authorize an operation.
    pub project_id: Option<String>,
    pub checkout: Option<PathBuf>,
    pub checkout_key: Option<String>,
    /// Read-generation this job was issued under; `0` for writes and
    /// project-level calls (writes must always report back).
    pub generation: u64,
    pub req: Req,
}

impl Job {
    pub fn read(project_id: &str, checkout: PathBuf, checkout_key: String, req: Req) -> Job {
        Job {
            project_id: Some(project_id.to_string()),
            checkout: Some(checkout),
            checkout_key: Some(checkout_key),
            generation: 0,
            req,
        }
    }
    pub fn project(req: Req) -> Job {
        Job {
            project_id: None,
            checkout: None,
            checkout_key: None,
            generation: 0,
            req,
        }
    }
    pub fn write(project_id: &str, checkout: PathBuf, checkout_key: String, req: Req) -> Job {
        Job {
            project_id: Some(project_id.to_string()),
            checkout: Some(checkout),
            checkout_key: Some(checkout_key),
            generation: 0,
            req,
        }
    }
}

pub struct Backend {
    tx: Option<SyncSender<Job>>,
    /// Set by `shutdown`: the worker drains accepted MUTATING jobs and
    /// skips queued reads instead of blocking exit behind a full queue.
    shutdown: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Set when the bounded queue refused a job; surfaced as a notice.
    pub overloaded: bool,
}

impl Backend {
    pub fn new(handle: AppHandle<State>) -> Backend {
        let (tx, rx) = sync_channel::<Job>(PENDING_CAP);
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        let worker = std::thread::Builder::new()
            .name("goko-editor-backend".into())
            .spawn(move || worker(rx, handle, flag))
            .expect("spawn editor backend worker");
        Backend {
            tx: Some(tx),
            shutdown,
            worker: Some(worker),
            overloaded: false,
        }
    }

    /// Stop the worker and wait for it: refuse new jobs, let every accepted
    /// MUTATING job (Save/AddProject/SetSetting/Git writes) finish so no
    /// write is orphaned, skip queued reads so exit never waits through a
    /// full 128-job read backlog, then join the thread. Idempotent; `Drop`
    /// calls it too, so a `Backend` that is simply abandoned still drains.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    /// Enqueue `job`. Returns `false` when the bounded queue refused it —
    /// callers must then clear the busy/loading flag they set, or the UI
    /// sticks on "saving".
    pub fn send(&mut self, job: Job) -> bool {
        let Some(tx) = &self.tx else { return false };
        match tx.try_send(job) {
            Ok(()) => {
                self.overloaded = false;
                true
            }
            Err(std::sync::mpsc::TrySendError::Full(job)) => {
                self.overloaded = true;
                // Hand the refused job back through the error channel shape
                // callers expect: report it as a failed reply is not possible
                // (no id was consumed by the worker), so surface via flag.
                let _ = job;
                false
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                self.tx = None;
                false
            }
        }
    }
}

/// Enqueue `job`; `false` means the bounded queue is full — the caller owns
/// unwinding whatever busy state it set before calling.
pub fn enqueue(backend: &mut Backend, job: Job) -> bool {
    backend.send(job)
}

/// Jobs that mutate disk, storage or Git — the only ones worth finishing
/// during shutdown. Every read/validation job is safe to drop: its reply
/// would land on a UI that is going away anyway.
fn is_mutating(req: &Req) -> bool {
    matches!(
        req,
        Req::AddProject(_)
            | Req::SetSetting { .. }
            | Req::SaveFile { .. }
            | Req::Stage { .. }
            | Req::Unstage { .. }
            | Req::Discard { .. }
            | Req::Commit { .. }
            | Req::Push
    )
}

fn worker(rx: std::sync::mpsc::Receiver<Job>, handle: AppHandle<State>, shutdown: Arc<AtomicBool>) {
    // The worker owns its storage connection: `Storage` is `Send` but not
    // `Sync`, and the UI thread keeps its own for cheap settings reads.
    // Unit tests must never touch the real GOKO_HOME — `run_job` tests pass
    // an explicit temp `Storage` instead.
    #[cfg(not(test))]
    let storage = home::home_dir().ok().map(|home| Storage::open(&home));
    #[cfg(test)]
    let storage: Option<Storage> = None;
    while let Ok(job) = rx.recv() {
        if shutdown.load(Ordering::SeqCst) && !is_mutating(&job.req) {
            // Shutdown drains the queue but never starts new reads — a
            // 128-job backlog must not delay exit behind git calls. The
            // reply still posts an Err so any still-mounted UI unwinds
            // its loading flags deterministically.
            let reply = Reply {
                generation: job.generation,
                req: job.req.clone(),
                checkout_key: job.checkout_key.clone(),
                result: Err("editor backend is shutting down".to_string()),
            };
            let h = handle.clone();
            handle.update(move |s: &mut State, _tx| {
                s.editor.apply_reply(reply);
                let _ = &h;
            });
            continue;
        }
        let reply = run_job(&storage, &job);
        let h = handle.clone();
        handle.update(move |s: &mut State, _tx| {
            s.editor.apply_reply(reply);
            // The Changes section keeps its patch-pane selection valid —
            // the desktop's "select first change" effect, run where a
            // `&mut State` exists (replies land outside the render pass).
            if s.editor.section == Section::Changes {
                crate::editor::changes::ensure_selection(s);
            }
            let _ = &h;
        });
    }
}

fn run_job(storage: &Option<Storage>, job: &Job) -> Reply {
    let checkout_key = job.checkout_key.clone();
    let result = match &job.req {
        Req::Projects => storage_req(storage, |store| {
            projects::list_projects(store).map(ReplyData::Projects)
        }),
        Req::AddProject(path) => storage_req(storage, |store| {
            projects::add_project(store, path).map(ReplyData::Added)
        }),
        Req::Worktrees(project_id) => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::list(std::path::Path::new(&root)).map(ReplyData::Worktrees)
        }),
        Req::GetSetting { key } => storage_req(storage, |store| {
            store.get_setting(key).map(ReplyData::Setting)
        }),
        Req::SetSetting { key, value } => storage_req(storage, |store| {
            store.set_setting(key, value).map(|_| ReplyData::Ok)
        }),
        Req::ExternalOpen {
            project_id,
            checkout_path,
            path,
            line,
        } => storage_req(storage, |store| {
            // Same validation every checkout job gets, plus the once-only
            // open payload — wrong project/path/checkouts fail here.
            let checkout = projects::git_checkout(store, project_id, checkout_path)?;
            Ok(ReplyData::ExternalOpen {
                checkout,
                path: path.clone(),
                line: *line,
            })
        }),
        Req::Highlight {
            path,
            revision,
            text,
        } => Ok(ReplyData::Highlight(syntax::highlight_revision(
            *revision,
            syntax::language_for_path(path),
            text,
        ))),
        _ => {
            let (Some(project_id), Some(checkout)) = (&job.project_id, &job.checkout) else {
                return Reply {
                    generation: job.generation,
                    req: job.req.clone(),
                    checkout_key,
                    result: Err("checkout job missing project or path identity".into()),
                };
            };
            run_checkout(storage, project_id, checkout, &job.req)
        }
    };
    Reply {
        generation: job.generation,
        req: job.req.clone(),
        checkout_key,
        result,
    }
}

fn storage_req<T>(
    storage: &Option<Storage>,
    f: impl FnOnce(&app_core::store::Store) -> Result<T, BridgeError>,
) -> Result<T, String> {
    let Some(storage) = storage else {
        return Err("storage is unavailable".into());
    };
    let store = storage.get().map_err(|e| e.to_string())?;
    f(store).map_err(|e| e.to_string())
}

/// Resolve the job's checkout identity against the registered project list
/// immediately before the Git call: `projects::git_checkout` re-checks the
/// project exists, canonicalizes the path (root or listed worktree) and
/// verifies a real non-bare worktree. A removed project, unregistered path,
/// foreign worktree or symlink swap fails here — never at the Git layer.
fn run_checkout(
    storage: &Option<Storage>,
    project_id: &str,
    checkout: &Path,
    req: &Req,
) -> Result<ReplyData, String> {
    let checkout = storage_req(storage, |store| {
        projects::git_checkout(store, project_id, &checkout.display().to_string())
    })?;
    let checkout = &checkout;
    match req {
        Req::Changes => git_view::changes(checkout)
            .map(ReplyData::Changes)
            .map_err(|e| e.to_string()),
        Req::Diff {
            layer,
            path,
            commit,
        } => git_view::diff(checkout, layer, path.as_deref(), commit.as_deref())
            .map(ReplyData::Diff)
            .map_err(|e| e.to_string()),
        Req::File { path, .. } | Req::ReloadFile { path, .. } => git_view::file(checkout, path)
            .map(ReplyData::File)
            .map_err(|e| e.to_string()),
        Req::History { skip, branch } => git_view::history(checkout, *skip, branch.as_deref())
            .map(ReplyData::History)
            .map_err(|e| e.to_string()),
        Req::Branches => git_view::branches(checkout)
            .map(ReplyData::Branches)
            .map_err(|e| e.to_string()),
        Req::FileList => git_view::file_list(checkout)
            .map(ReplyData::FileList)
            .map_err(|e| e.to_string()),
        Req::Search { query } | Req::GoToSearch { query } => git_view::search(checkout, query)
            .map(ReplyData::Search)
            .map_err(|e| e.to_string()),
        Req::ConfigFiles { paths } => Ok(config_aliases(checkout, paths)),
        Req::Stashes => git_view::stashes(checkout)
            .map(ReplyData::Stashes)
            .map_err(|e| e.to_string()),
        Req::StashDiff { stash } => git_view::stash_diff(checkout, stash)
            .map(ReplyData::StashDiff)
            .map_err(|e| e.to_string()),
        Req::SaveFile {
            path,
            expected,
            text,
        } => git_view::save_file(checkout, path, expected, text)
            .map(ReplyData::Saved)
            .map_err(|e| e.to_string()),
        Req::Stage { paths } => git_write::stage(checkout, paths.as_deref())
            .map(|_| ReplyData::Ok)
            .map_err(|e| e.to_string()),
        Req::Unstage { paths } => git_write::unstage(checkout, paths.as_deref())
            .map(|_| ReplyData::Ok)
            .map_err(|e| e.to_string()),
        Req::Discard { layer, path } => git_write::discard(checkout, layer, path)
            .map(|_| ReplyData::Ok)
            .map_err(|e| e.to_string()),
        Req::Commit { message } => git_write::commit(checkout, message)
            .map(ReplyData::Commit)
            .map_err(|e| e.to_string()),
        Req::Push => git_write::push(checkout)
            .map(ReplyData::Push)
            .map_err(|e| e.to_string()),
        Req::Projects
        | Req::AddProject(_)
        | Req::Worktrees(_)
        | Req::GetSetting { .. }
        | Req::SetSetting { .. }
        | Req::ExternalOpen { .. }
        | Req::Highlight { .. } => Err("non-checkout request on checkout lane".into()),
    }
}

/// The desktop `loadAliases` scan, worker-side: walk the ordered candidates
/// nearest-first and return the `paths` aliases of the FIRST config that
/// reads as text and parses to at least one alias. Missing, binary,
/// unreadable and alias-less (or malformed) configs are skipped — matching
/// main, where only `aliases.length` truthy ends the loop.
fn config_aliases(checkout: &Path, paths: &[String]) -> ReplyData {
    for path in paths {
        let Ok(file) = git_view::file(checkout, path) else {
            continue;
        };
        if file.kind != "text" {
            continue;
        }
        let Some(text) = &file.text else {
            continue;
        };
        let aliases = crate::editor::go_to_definition::parse_tsconfig_paths(text, path);
        if !aliases.is_empty() {
            return ReplyData::ConfigAliases(aliases);
        }
    }
    ReplyData::ConfigAliases(Vec::new())
}

impl EditorState {
    /// Issue a checkout-scoped read. Each kind gets a fresh generation so a
    /// slower earlier reply for the same checkout can't overwrite newer data.
    /// Returns `false` when the bounded queue refused the job — the caller's
    /// loading flag stays set only when this returns `true`.
    pub fn enqueue_read(&mut self, checkout: PathBuf, key: String, req: Req) -> bool {
        let Some(pid) = self.project_id.clone() else {
            return false;
        };
        let kind = kind_key(&req);
        let generation = self.req_gens.entry(kind).or_insert(0);
        *generation += 1;
        let generation = *generation;
        let mut job = Job::read(&pid, checkout, key, req);
        job.generation = generation;
        self.backend.send(job)
    }

    /// Issue a checkout-scoped write on the same serialized lane. Writes are
    /// never generation-gated: every write must report back so busy flags
    /// and draft state resolve. `false` = queue full; caller clears busy.
    pub fn enqueue_write(&mut self, checkout: PathBuf, key: String, req: Req) -> bool {
        let Some(pid) = self.project_id.clone() else {
            return false;
        };
        self.backend.send(Job::write(&pid, checkout, key, req))
    }

    /// Rebuild the FileTree model from `files.paths` filtered by the filter
    /// text (substring on the basename, like the desktop's filename filter).
    pub fn rebuild_file_model(&mut self) {
        let filter = self.files.filter.text.to_lowercase();
        let paths: Vec<&str> = self
            .files
            .paths
            .iter()
            .map(String::as_str)
            .filter(|p| {
                filter.is_empty()
                    || p.rsplit('/')
                        .next()
                        .unwrap_or(p)
                        .to_lowercase()
                        .contains(&filter)
                    || p.to_lowercase().contains(&filter)
            })
            .collect();
        self.files.model = rune_kit::FileTree::new()
            .dirs_first(true)
            .build(paths.into_iter().map(|p| (p, ())));
        self.files.model_checkout = self.checkout_key.clone();
    }

    /// Files became the active surface: any in-flight ACP generate belonged
    /// to the Changes commit panel, so cancel it with the CURRENT (still
    /// pre-transition) project/checkout identity before the section flips.
    fn activate_files(&mut self) {
        if self.section == Section::Changes {
            self.acp.cancel(&self.project_id, &self.checkout);
        }
        self.section = Section::Files;
        self.visited.insert(Section::Files);
    }

    fn install_doc(&mut self, path: String, file: GitViewFile, line: Option<u64>) {
        // Same file already open → just queue the reveal, no reload
        // (external open-file contract). The doc is untouched, so a pending
        // reload stays valid.
        if self.doc.as_ref().is_some_and(|d| d.path == path) {
            if let (Some(line), Some(doc)) = (line, self.doc.as_mut()) {
                doc.reveal_line = Some(line);
            }
            self.activate_files();
            return;
        }
        let mut doc = crate::editor::draft::OpenDoc::new(path, &file);
        doc.reveal_line = line;
        // Fresh document: arm the highlight debounce so the initial text is
        // coloured once the quiet period passes.
        doc.highlight_dirty_at = Some(std::time::Instant::now());
        // The draft guard already resolved before this reply was applied, so
        // replacing the slot here cannot drop unsaved work. A pending reload
        // targeted the OLD doc — never let it land on the new one.
        self.doc_reload = None;
        self.doc = Some(doc);
        self.activate_files();
    }

    /// `Req::ReloadFile` reply — the guard dialog's "Reload from disk":
    /// replace the open doc with the fresh disk snapshot (baseline AND
    /// buffer, like the desktop clearing the draft and reopening). Applies
    /// only while the request is still current: token not superseded, doc
    /// still the one it was issued for (path), buffer untouched since issue
    /// (revision — an edit during the flight bumps it and drops the reply),
    /// and checkout scope already matched in `apply_reply`. Failure or
    /// staleness preserves the old buffer and its error state.
    fn apply_reload(&mut self, path: &str, token: u64, revision: u64, file: GitViewFile) {
        if self.doc_reload != Some(token) {
            return;
        }
        self.doc_reload = None;
        if !self
            .doc
            .as_ref()
            .is_some_and(|d| d.path == path && d.code.revision == revision)
        {
            return;
        }
        if file.kind != "text" || file.text.is_none() {
            self.notice = Some(Notice {
                text: format!("cannot reload '{path}': the file is not UTF-8 text"),
                stale: false,
            });
            return;
        }
        let mut doc = crate::editor::draft::OpenDoc::new(path.to_string(), &file);
        doc.reveal_line = self.doc.as_ref().and_then(|d| d.reveal_line);
        doc.highlight_dirty_at = Some(std::time::Instant::now());
        self.doc = Some(doc);
        self.sync_quit_dirty();
    }

    /// Kick the reads the active section needs. Called on section activation,
    /// checkout selection and manual refresh. A refused `enqueue_read` must
    /// unwind deterministically: the loading flag it guarded stays set only
    /// when the job was accepted, and the refusal surfaces where the
    /// section's own error row renders it.
    pub fn refresh_active(&mut self) {
        let Some(checkout) = self.checkout.clone() else {
            return;
        };
        let Some(key) = self.checkout_key.clone() else {
            return;
        };
        match self.section {
            Section::Changes => {
                self.changes.loading = true;
                self.changes.error = None;
                if !self.enqueue_read(checkout.clone(), key.clone(), Req::Changes) {
                    self.changes.loading = false;
                    queue_full_refresh(
                        &mut self.changes.error,
                        &mut self.notice,
                        self.changes.data.is_some(),
                    );
                }
                // Patch pane follows the selected layer/path.
                if let (Some(layer), Some(path)) =
                    (self.changes.layer.clone(), self.changes.patch_path.clone())
                {
                    self.changes.patch_loading = true;
                    if !self.enqueue_read(
                        checkout,
                        key,
                        Req::Diff {
                            layer,
                            path: Some(path),
                            commit: None,
                        },
                    ) {
                        self.changes.patch_loading = false;
                        queue_full_refresh(
                            &mut self.changes.error,
                            &mut self.notice,
                            self.changes.patch.is_some(),
                        );
                    }
                }
            }
            Section::Files => {
                if self.files.model_checkout.as_deref() != Some(key.as_str())
                    || self.files.paths.is_empty()
                {
                    self.files.loading = true;
                    self.files.error = None;
                    if !self.enqueue_read(checkout, key, Req::FileList) {
                        self.files.loading = false;
                        queue_full_refresh(
                            &mut self.files.error,
                            &mut self.notice,
                            !self.files.paths.is_empty(),
                        );
                    }
                }
            }
            Section::History => {
                if self.history.checkout.as_deref() != Some(key.as_str())
                    || self.history.commits.is_empty()
                {
                    self.history.loading = true;
                    self.history.error = None;
                    if !self.enqueue_read(
                        checkout.clone(),
                        key.clone(),
                        Req::History {
                            skip: 0,
                            branch: self.history.branch.clone(),
                        },
                    ) {
                        self.history.loading = false;
                        queue_full_refresh(
                            &mut self.history.error,
                            &mut self.notice,
                            !self.history.commits.is_empty(),
                        );
                    }
                    // Branches carry no loading flag and their Err replies
                    // are already ignored — a refusal needs no unwind.
                    self.enqueue_read(checkout, key, Req::Branches);
                }
            }
            Section::Stashes => {
                if self.stashes.checkout.as_deref() != Some(key.as_str()) {
                    self.stashes.loading = true;
                    self.stashes.error = None;
                    if !self.enqueue_read(checkout, key, Req::Stashes) {
                        self.stashes.loading = false;
                        queue_full_refresh(
                            &mut self.stashes.error,
                            &mut self.notice,
                            !self.stashes.stashes.is_empty(),
                        );
                    }
                }
            }
            // Servers reads hub status snapshots from `lsp::poll`, never
            // the worker lane.
            Section::Servers => {}
        }
    }

    /// Consume `history.layout_dirty`: persist the current layout JSON once.
    /// Optimistic-clear + in-flight flag keep this bounded — a refused queue
    /// simply retries on a later wake, a reply (ok or err) releases the flag.
    /// Called from `AppSpec::poll`; returns `false` (no visible change).
    pub fn poll_layout_save(&mut self) {
        if !self.history.layout_dirty || self.history.layout_save_pending {
            return;
        }
        let value = self.history.layout.to_json();
        if enqueue(
            &mut self.backend,
            Job::project(Req::SetSetting {
                key: crate::editor::history_layout::HISTORY_LAYOUT_KEY.to_string(),
                value,
            }),
        ) {
            self.history.layout_dirty = false;
            self.history.layout_save_pending = true;
        }
    }

    /// Initial project list + worktree list for the selected project.
    pub fn refresh_projects(&mut self) {
        self.projects_loading = true;
        self.backend.send(Job::project(Req::Projects));
        if let Some(pid) = self.project_id.clone() {
            self.worktrees_loading = true;
            self.backend.send(Job::project(Req::Worktrees(pid)));
        }
    }
}

/// Request-kind key for generation tracking; writes and project calls map
/// to kinds that never carry a generation.
fn kind_key(req: &Req) -> &'static str {
    match req {
        Req::Changes => "changes",
        Req::Diff {
            commit: Some(_), ..
        } => "commitdiff",
        Req::Diff { .. } => "diff",
        Req::History { .. } => "history",
        Req::Branches => "branches",
        Req::FileList => "filelist",
        Req::Search { .. } => "search",
        Req::GoToSearch { .. } => "gotosarch",
        Req::ConfigFiles { .. } => "configfiles",
        Req::Highlight { .. } => "highlight",
        Req::File { .. } => "file",
        Req::ReloadFile { .. } => "reloadfile",
        Req::Stashes => "stashes",
        Req::StashDiff { .. } => "stashdiff",
        Req::ExternalOpen { .. } => "externalopen",
        _ => "other",
    }
}

/// Queue-refusal unwind for `refresh_active`, mirroring the reply `Err`
/// arms' split: the section `error` row while it has no data, a stale
/// notice over cached data that stays visible.
fn queue_full_refresh(error: &mut Option<String>, notice: &mut Option<Notice>, has_data: bool) {
    let text = "Backend queue is full — try again".to_string();
    if has_data {
        *notice = Some(Notice { text, stale: true });
    } else {
        *error = Some(text);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::state::{EditorState, checkout_key};
    use app_core::git_view::GitViewFile;
    use app_core::projects::ProjectView;
    use app_core::store::Project;

    fn editor() -> EditorState {
        EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle())
    }

    /// Registered project + canonical checkout key, as `select_checkout`
    /// would leave them.
    fn select(e: &mut EditorState) {
        e.project_id = Some("p1".into());
        e.checkout = Some(PathBuf::from("/tmp/co"));
        e.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
    }

    fn text_file(text: &str) -> GitViewFile {
        GitViewFile {
            path: "a.txt".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    fn reply(
        req: Req,
        generation: u64,
        key: Option<String>,
        result: Result<ReplyData, String>,
    ) -> Reply {
        Reply {
            req,
            generation,
            checkout_key: key,
            result,
        }
    }

    #[test]
    fn stale_generation_reply_is_dropped() {
        let mut e = editor();
        select(&mut e);
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        // Two reads of the same kind: the second supersedes the first.
        assert!(e.enqueue_read(checkout.clone(), key.clone(), Req::Changes));
        assert!(e.enqueue_read(checkout, key.clone(), Req::Changes));
        // The superseded generation-1 reply must not touch state.
        e.apply_reply(reply(Req::Changes, 1, Some(key.clone()), Err("old".into())));
        assert!(e.changes.error.is_none());
        // The current generation applies.
        e.apply_reply(reply(Req::Changes, 2, Some(key), Err("new".into())));
        assert_eq!(e.changes.error.as_deref(), Some("new"));
    }

    #[test]
    fn foreign_checkout_reply_is_dropped() {
        let mut e = editor();
        select(&mut e);
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        assert!(e.enqueue_read(checkout, key.clone(), Req::Changes));
        e.apply_reply(reply(
            Req::Changes,
            1,
            Some("p9\u{1f}/other".into()),
            Err("foreign".into()),
        ));
        assert!(e.changes.error.is_none());
    }

    #[test]
    fn stage_unstage_discard_success_clears_busy() {
        let mut e = editor();
        select(&mut e);
        let key = e.checkout_key.clone().unwrap();
        // Each write kind: busy arms, the Ok reply must clear it (and any
        // obsolete error) so the next action is permitted.
        for (req, kind) in [
            (Req::Stage { paths: None }, "stage"),
            (Req::Unstage { paths: None }, "unstage"),
            (
                Req::Discard {
                    layer: "working".into(),
                    path: "a.txt".into(),
                },
                "discard",
            ),
        ] {
            e.changes.busy = Some(kind);
            e.changes.error = Some("obsolete".into());
            e.apply_reply(reply(req, 0, Some(key.clone()), Ok(ReplyData::Ok)));
            assert_eq!(e.changes.busy, None, "{kind} success clears busy");
            assert_eq!(e.changes.error, None, "{kind} success clears stale error");
            // The gate is open: a subsequent write re-arms busy normally.
            crate::editor::changes::stage_paths(&mut e, None);
            assert_eq!(e.changes.busy, Some("stage"));
            e.changes.busy = None;
        }
    }

    #[test]
    fn stage_error_clears_busy_and_keeps_error() {
        let mut e = editor();
        select(&mut e);
        let key = e.checkout_key.clone().unwrap();
        e.changes.busy = Some("stage");
        e.apply_reply(reply(
            Req::Stage { paths: None },
            0,
            Some(key),
            Err("denied".into()),
        ));
        assert_eq!(e.changes.busy, None, "error arm already clears busy");
        assert_eq!(e.changes.error.as_deref(), Some("denied"));
        assert!(e.notice.is_some());
    }

    #[test]
    fn foreign_checkout_write_reply_preserves_busy() {
        let mut e = editor();
        select(&mut e);
        e.changes.busy = Some("stage");
        // A Stage reply scoped to a different checkout drops before the
        // match — it must not clear the current checkout's busy flag.
        e.apply_reply(reply(
            Req::Stage { paths: None },
            0,
            Some("p9\u{1f}/other".into()),
            Ok(ReplyData::Ok),
        ));
        assert_eq!(e.changes.busy, Some("stage"));
    }

    #[test]
    fn write_reply_always_applies() {
        let mut e = editor();
        select(&mut e);
        let key = e.checkout_key.clone().unwrap();
        e.doc = Some(OpenDoc::new("a.txt", &text_file("hi")));
        e.doc.as_mut().unwrap().saving = true;
        // Generation 0 marks a write: even though reads are gated, the save
        // reply must land so `saving` can never stick.
        e.apply_reply(reply(
            Req::SaveFile {
                path: "a.txt".into(),
                expected: "hi".into(),
                text: "hi".into(),
            },
            0,
            Some(key),
            Ok(ReplyData::Saved(text_file("hi"))),
        ));
        let doc = e.doc.as_ref().unwrap();
        assert!(!doc.saving);
        assert!(doc.save_error.is_none());
    }

    #[test]
    fn enqueue_requires_registered_project() {
        let mut e = editor();
        // No project selected: reads and writes are refused before a job
        // can reach the worker with an arbitrary path.
        assert!(!e.enqueue_read(PathBuf::from("/tmp/co"), "k".into(), Req::Changes));
        assert!(!e.enqueue_write(
            PathBuf::from("/tmp/co"),
            "k".into(),
            Req::Commit {
                message: "m".into()
            }
        ));
    }

    #[test]
    fn add_project_reply_never_switches_selection() {
        let mut e = editor();
        select(&mut e);
        let key = e.checkout_key.clone().unwrap();
        let added = ProjectView {
            project: Project {
                id: "p2".into(),
                name: "P2".into(),
                root: "/tmp/p2".into(),
                created_at: 0,
            },
            missing: false,
        };
        e.apply_reply(reply(
            Req::AddProject("/tmp/p2".into()),
            0,
            None,
            Ok(ReplyData::Added(added)),
        ));
        // The project list grew but the active identity is untouched — an
        // added project must never auto-switch away from a live checkout.
        assert_eq!(e.project_id.as_deref(), Some("p1"));
        assert_eq!(e.checkout_key.as_deref(), Some(key.as_str()));
        assert!(e.projects.iter().any(|p| p.project.id == "p2"));
    }

    #[test]
    fn same_file_reply_reveals_without_replacing_draft() {
        let mut e = editor();
        select(&mut e);
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.doc.as_mut().unwrap().code.edit.text = "dirty edit".into();
        e.doc_load = Some(9);
        assert!(e.enqueue_read(
            checkout,
            key.clone(),
            Req::File {
                path: "a.txt".into(),
                token: 9,
                line: Some(7)
            }
        ));
        e.apply_reply(reply(
            Req::File {
                path: "a.txt".into(),
                token: 9,
                line: Some(7),
            },
            1,
            Some(key),
            Ok(ReplyData::File(text_file("orig"))),
        ));
        let doc = e.doc.as_ref().unwrap();
        // Same-path replies only queue the reveal — the dirty buffer and
        // its original are both preserved (external open-file contract).
        assert_eq!(doc.reveal_line, Some(7));
        assert_eq!(doc.code.edit.text, "dirty edit");
        assert_eq!(doc.original, "orig");
        assert_eq!(e.section, crate::editor::state::Section::Files);
    }

    #[test]
    fn stale_file_token_reply_is_ignored() {
        let mut e = editor();
        select(&mut e);
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        e.doc_load = Some(9);
        assert!(e.enqueue_read(
            checkout,
            key.clone(),
            Req::File {
                path: "a.txt".into(),
                token: 9,
                line: None
            }
        ));
        // A reply for a superseded load token must not install a doc.
        e.apply_reply(reply(
            Req::File {
                path: "a.txt".into(),
                token: 4,
                line: None,
            },
            1,
            Some(key),
            Ok(ReplyData::File(text_file("orig"))),
        ));
        assert!(e.doc.is_none());
    }

    // -- go-to config lookup -------------------------------------------------

    fn config_json(pattern: &str, target: &str) -> String {
        format!("{{\"compilerOptions\":{{\"paths\":{{\"{pattern}\":[\"{target}\"]}}}}}}")
    }

    fn config_aliases_of(data: ReplyData) -> Vec<crate::editor::go_to_definition::PathAlias> {
        let ReplyData::ConfigAliases(aliases) = data else {
            panic!("expected ConfigAliases, got {data:?}")
        };
        aliases
    }

    #[test]
    fn config_aliases_skips_unreadable_and_aliasless_configs() {
        use app_core::test_support::TempDir;
        let dir = TempDir::new("goto-config");
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        // Nearest candidates: missing file, malformed JSON, and a readable
        // config without `paths` — the desktop skips all three because only
        // a non-empty alias list ends its scan.
        std::fs::write(root.join("src/deep/tsconfig.json"), "{ not json").unwrap();
        std::fs::write(root.join("src/tsconfig.json"), "{\"compilerOptions\":{}}").unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            config_json("@app/*", "src/app/*"),
        )
        .unwrap();
        let paths: Vec<String> = [
            "src/deep/tsconfig.json",
            "src/deep/jsconfig.json",
            "src/tsconfig.json",
            "src/jsconfig.json",
            "tsconfig.json",
            "jsconfig.json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let aliases = config_aliases_of(config_aliases(&root, &paths));
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].pattern, "@app/*");
        assert_eq!(aliases[0].targets, vec!["src/app/*".to_string()]);
    }

    #[test]
    fn config_aliases_jsconfig_fallback_and_nearest_wins() {
        use app_core::test_support::TempDir;
        // jsconfig.json answers when tsconfig is absent.
        let dir = TempDir::new("goto-jsconfig");
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(
            dir.path().join("jsconfig.json"),
            config_json("@lib/*", "lib/*"),
        )
        .unwrap();
        let aliases = config_aliases_of(config_aliases(
            &root,
            &["tsconfig.json".into(), "jsconfig.json".into()],
        ));
        assert_eq!(aliases[0].pattern, "@lib/*");
        // The nearest config WITH aliases shadows every ancestor.
        let dir = TempDir::new("goto-nearest");
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(
            root.join("sub/tsconfig.json"),
            config_json("@sub/*", "sub/*"),
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            config_json("@app/*", "src/app/*"),
        )
        .unwrap();
        let aliases = config_aliases_of(config_aliases(
            &root,
            &["sub/tsconfig.json".into(), "tsconfig.json".into()],
        ));
        assert_eq!(aliases[0].pattern, "@sub/*");
    }

    #[test]
    fn config_aliases_empty_when_no_config_defines_paths() {
        use app_core::test_support::TempDir;
        let dir = TempDir::new("goto-noconfig");
        let aliases = config_aliases(
            &dir.path().canonicalize().unwrap(),
            &["tsconfig.json".into(), "jsconfig.json".into()],
        );
        assert!(config_aliases_of(aliases).is_empty());
    }

    #[test]
    fn config_aliases_walks_past_the_old_hidden_cap() {
        use app_core::test_support::TempDir;
        // 25 candidates — far beyond the removed `.take(8)` UI cutoff; the
        // worker walks the whole ordered list like the desktop.
        let dir = TempDir::new("goto-deep");
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("tsconfig.json"), config_json("@deep/*", "deep/*")).unwrap();
        let mut paths: Vec<String> = (0..12)
            .flat_map(|i| [format!("d{i}/tsconfig.json"), format!("d{i}/jsconfig.json")])
            .collect();
        paths.push("tsconfig.json".into());
        paths.push("jsconfig.json".into());
        assert_eq!(paths.len(), 26);
        let aliases = config_aliases_of(config_aliases(&root, &paths));
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].pattern, "@deep/*");
    }

    fn pending_import() -> crate::editor::state::GoToPending {
        crate::editor::state::GoToPending {
            from_path: "src/a.ts".into(),
            stage: crate::editor::state::GoToStage::AwaitConfigs {
                specifier: "@app/util".into(),
            },
        }
    }

    fn alias() -> crate::editor::go_to_definition::PathAlias {
        crate::editor::go_to_definition::PathAlias {
            pattern: "@app/*".into(),
            targets: vec!["src/app/*".into()],
        }
    }

    fn config_reply(
        generation: u64,
        key: Option<String>,
        aliases: Vec<crate::editor::go_to_definition::PathAlias>,
    ) -> Reply {
        reply(
            Req::ConfigFiles {
                paths: vec!["tsconfig.json".into()],
            },
            generation,
            key,
            Ok(ReplyData::ConfigAliases(aliases)),
        )
    }

    #[test]
    fn config_files_reply_resolves_pending_import() {
        let mut e = editor();
        select(&mut e);
        e.section = Section::Files; // go-to lookups live on the Files surface
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        e.files.paths = vec!["src/a.ts".into(), "src/app/util.ts".into()];
        e.doc = Some(OpenDoc::new("src/a.ts", &text_file("x")));
        e.go_to = Some(pending_import());
        assert!(e.enqueue_read(
            checkout,
            key.clone(),
            Req::ConfigFiles {
                paths: vec!["tsconfig.json".into()]
            }
        ));
        e.apply_reply(config_reply(1, Some(key), vec![alias()]));
        assert!(e.go_to.is_none(), "lookup consumed by its reply");
        assert!(
            matches!(
                e.pending_apply,
                Some(crate::editor::navigation::PendingNav::OpenDoc { ref path, .. })
                    if path == "src/app/util.ts"
            ),
            "alias resolved to the target: {:?}",
            e.pending_apply
        );
    }

    #[test]
    fn config_files_stale_generation_and_foreign_checkout_drop() {
        let mut e = editor();
        select(&mut e);
        e.section = Section::Files;
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        e.files.paths = vec!["src/app/util.ts".into()];
        e.doc = Some(OpenDoc::new("src/a.ts", &text_file("x")));
        e.go_to = Some(pending_import());
        // Two lookups: the second supersedes the first generation.
        e.enqueue_read(
            checkout.clone(),
            key.clone(),
            Req::ConfigFiles {
                paths: vec!["tsconfig.json".into()],
            },
        );
        e.enqueue_read(
            checkout,
            key.clone(),
            Req::ConfigFiles {
                paths: vec!["tsconfig.json".into()],
            },
        );
        // The superseded gen-1 reply drops: pending survives, nothing opens.
        e.apply_reply(config_reply(1, Some(key.clone()), vec![alias()]));
        assert!(e.go_to.is_some());
        assert!(e.pending_apply.is_none());
        // A foreign-checkout reply drops the same way.
        e.apply_reply(config_reply(
            2,
            Some("p9\u{1f}/other".into()),
            vec![alias()],
        ));
        assert!(e.go_to.is_some());
        assert!(e.pending_apply.is_none());
        // The current generation resolves.
        e.apply_reply(config_reply(2, Some(key), vec![alias()]));
        assert!(e.go_to.is_none());
        assert!(e.pending_apply.is_some());
    }

    // -- external open + refresh refusal ------------------------------------

    fn external_open_reply(
        project_id: &str,
        checkout: &str,
        path: &str,
        line: Option<u64>,
    ) -> Reply {
        reply(
            Req::ExternalOpen {
                project_id: project_id.into(),
                checkout_path: checkout.into(),
                path: path.into(),
                line,
            },
            0,
            None,
            Ok(ReplyData::ExternalOpen {
                checkout: PathBuf::from(checkout),
                path: path.into(),
                line,
            }),
        )
    }

    #[test]
    fn external_open_queues_one_typed_pending_and_refuses_overwrite() {
        let mut e = editor();
        select(&mut e);
        e.apply_reply(external_open_reply("p1", "/tmp/co", "src/a.ts", Some(3)));
        assert!(
            matches!(
                e.pending_apply,
                Some(crate::editor::navigation::PendingNav::ExternalOpen {
                    ref project_id,
                    ref path,
                    ..
                }) if project_id == "p1" && path == "src/a.ts"
            ),
            "external open queued with its identity: {:?}",
            e.pending_apply
        );
        // A second request before poll is refused loudly — the first
        // pending is never overwritten.
        e.apply_reply(external_open_reply("p1", "/tmp/co", "src/b.ts", None));
        assert!(
            matches!(
                e.pending_apply,
                Some(crate::editor::navigation::PendingNav::ExternalOpen {
                    ref path, ..
                }) if path == "src/a.ts"
            ),
            "first request still pending: {:?}",
            e.pending_apply
        );
        assert!(
            e.notice
                .as_ref()
                .is_some_and(|n| n.text.contains("external open")),
            "refusal surfaced: {:?}",
            e.notice
        );
    }

    #[test]
    fn external_open_refused_while_navigation_or_save_is_pending() {
        use crate::editor::navigation::{Guard, PendingNav};
        // Occupied pending_apply slot.
        let mut e = editor();
        select(&mut e);
        e.pending_apply = Some(PendingNav::OpenDoc {
            path: "x.ts".into(),
            line: None,
        });
        e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
        assert!(matches!(e.pending_apply, Some(PendingNav::OpenDoc { .. })));
        assert!(e.notice.is_some());
        // Occupied save-resume slot.
        let mut e = editor();
        select(&mut e);
        e.save_resume = Some(Guard {
            pending: PendingNav::OpenDoc {
                path: "y.ts".into(),
                line: None,
            },
            doc_path: "a.txt".into(),
            quitting: false,
        });
        e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
        assert!(e.pending_apply.is_none());
        assert!(e.save_resume.is_some(), "save resume untouched");
        assert!(e.notice.is_some());
        // Occupied draft guard.
        let mut e = editor();
        select(&mut e);
        e.guard = Some(Guard {
            pending: PendingNav::OpenDoc {
                path: "z.ts".into(),
                line: None,
            },
            doc_path: "a.txt".into(),
            quitting: false,
        });
        e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
        assert!(e.pending_apply.is_none());
        assert!(e.guard.is_some(), "guard untouched");
        assert!(e.notice.is_some());
    }

    #[test]
    fn refresh_active_refusal_unwinds_every_section_flag() {
        let mut e = editor();
        select(&mut e);
        // `project_id = None` makes every `enqueue_read` refuse, like a
        // full bounded queue.
        e.project_id = None;
        for section in [
            Section::Changes,
            Section::Files,
            Section::History,
            Section::Stashes,
        ] {
            e.section = section;
            e.refresh_active();
            let (loading, error) = match section {
                Section::Changes => (e.changes.loading, e.changes.error.is_some()),
                Section::Files => (e.files.loading, e.files.error.is_some()),
                Section::History => (e.history.loading, e.history.error.is_some()),
                Section::Stashes => (e.stashes.loading, e.stashes.error.is_some()),
                // Not in the iterated set; arm exists for exhaustiveness.
                Section::Servers => (false, false),
            };
            assert!(!loading, "{section:?} stuck loading after refusal");
            assert!(error, "{section:?} missing queue-full error");
        }
    }

    #[test]
    fn refresh_active_patch_refusal_unwinds_patch_loading() {
        let mut e = editor();
        select(&mut e);
        e.project_id = None;
        e.section = Section::Changes;
        e.changes.layer = Some("worktree".into());
        e.changes.patch_path = Some("a.txt".into());
        e.refresh_active();
        assert!(!e.changes.loading);
        assert!(!e.changes.patch_loading);
        assert!(e.changes.error.is_some());
    }

    #[test]
    fn refresh_active_refusal_with_cached_data_uses_stale_notice() {
        let mut e = editor();
        select(&mut e);
        e.project_id = None;
        e.section = Section::Files;
        // Cached paths + a stale model key force the FileList read; the
        // refusal must not paint an error over data that is still shown.
        e.files.paths = vec!["a.txt".into()];
        e.files.model_checkout = Some("other".into());
        e.refresh_active();
        assert!(!e.files.loading);
        assert!(e.files.error.is_none());
        assert!(
            e.notice.as_ref().is_some_and(|n| n.stale),
            "cached-data refusal surfaces a stale notice: {:?}",
            e.notice
        );
    }

    // -- Req::ReloadFile ----------------------------------------------------

    /// Issue a reload exactly like `navigation::request_doc_reload`, minus
    /// the `State` wrapper: bump `doc_seq`, enqueue under the live checkout,
    /// arm `doc_reload`. Returns `(token, revision)` for the reply.
    fn issue_reload(e: &mut EditorState) -> (u64, u64) {
        let doc = e.doc.as_ref().expect("open doc");
        let path = doc.path.clone();
        let revision = doc.code.revision;
        e.doc_seq += 1;
        let token = e.doc_seq;
        let checkout = e.checkout.clone().unwrap();
        let key = e.checkout_key.clone().unwrap();
        assert!(e.enqueue_read(
            checkout,
            key,
            Req::ReloadFile {
                path,
                token,
                revision,
            }
        ));
        e.doc_reload = Some(token);
        (token, revision)
    }

    fn reload_reply(
        e: &EditorState,
        token: u64,
        revision: u64,
        result: Result<ReplyData, String>,
    ) -> Reply {
        Reply {
            req: Req::ReloadFile {
                path: e.doc.as_ref().unwrap().path.clone(),
                token,
                revision,
            },
            generation: e.req_gens["reloadfile"],
            checkout_key: e.checkout_key.clone(),
            result,
        }
    }

    #[test]
    fn reload_file_replaces_doc_on_clean_match() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.doc.as_mut().unwrap().code.edit.text = "edited".into();
        let (token, revision) = issue_reload(&mut e);
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Ok(ReplyData::File(text_file("disk"))),
        ));
        let doc = e.doc.as_ref().unwrap();
        // Baseline AND buffer now come from disk — the desktop's
        // reloadDraft clears the draft and reopens.
        assert_eq!(doc.original, "disk");
        assert_eq!(doc.code.edit.text, "disk");
        assert!(!doc.dirty());
        assert_eq!(e.doc_reload, None);
    }

    #[test]
    fn reload_file_stale_token_is_dropped() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.doc.as_mut().unwrap().code.edit.text = "edited".into();
        let (token, revision) = issue_reload(&mut e);
        // A second reload supersedes the first.
        let (token2, _) = issue_reload(&mut e);
        assert_ne!(token, token2);
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Ok(ReplyData::File(text_file("disk"))),
        ));
        // First reply dropped: buffer still holds the user's edits.
        assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
        assert_eq!(e.doc_reload, Some(token2));
    }

    #[test]
    fn reload_file_edit_during_flight_preserves_buffer() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        let (token, revision) = issue_reload(&mut e);
        // The user typed while the read was in flight → revision moved on.
        let doc = e.doc.as_mut().unwrap();
        doc.code.edit.text = "edited".into();
        doc.code.text_edited();
        assert_ne!(doc.code.revision, revision);
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Ok(ReplyData::File(text_file("disk"))),
        ));
        assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
        assert_eq!(e.doc_reload, None);
    }

    #[test]
    fn reload_file_doc_switch_preserves_new_doc() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        let (token, revision) = issue_reload(&mut e);
        // A different doc was installed mid-flight (install_doc clears
        // doc_reload, so the reply must not land on it).
        e.doc = Some(OpenDoc::new("b.txt", &text_file("other")));
        e.doc_reload = None;
        let mut reply = reload_reply(&e, token, revision, Ok(ReplyData::File(text_file("disk"))));
        reply.req = Req::ReloadFile {
            path: "a.txt".into(),
            token,
            revision,
        };
        e.apply_reply(reply);
        assert_eq!(e.doc.as_ref().unwrap().path, "b.txt");
        assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "other");
    }

    #[test]
    fn reload_file_error_preserves_buffer_and_surfaces_notice() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.doc.as_mut().unwrap().code.edit.text = "edited".into();
        let (token, revision) = issue_reload(&mut e);
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Err("permission denied".into()),
        ));
        let doc = e.doc.as_ref().unwrap();
        assert_eq!(doc.code.edit.text, "edited");
        assert!(doc.dirty());
        assert_eq!(e.doc_reload, None);
        assert!(
            e.notice
                .as_ref()
                .is_some_and(|n| n.text.contains("permission denied"))
        );
    }

    #[test]
    fn reload_file_non_text_reply_preserves_buffer() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.doc.as_mut().unwrap().code.edit.text = "edited".into();
        let (token, revision) = issue_reload(&mut e);
        let mut binary = text_file("");
        binary.kind = "binary";
        binary.text = None;
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Ok(ReplyData::File(binary)),
        ));
        let doc = e.doc.as_ref().unwrap();
        assert_eq!(doc.code.edit.text, "edited");
        assert!(doc.dirty());
        assert!(
            e.notice
                .as_ref()
                .is_some_and(|n| n.text.contains("not UTF-8"))
        );
    }

    #[test]
    fn normal_file_reply_does_not_block_pending_reload() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        // A same-path File reply takes the reveal fast path — it must not
        // consume or invalidate the in-flight reload.
        e.doc_load = Some(99);
        let file_gen = e.req_gens.get("file").copied().unwrap_or(0) + 1;
        e.req_gens.insert("file", file_gen);
        e.apply_reply(reply(
            Req::File {
                path: "a.txt".into(),
                token: 99,
                line: Some(3),
            },
            file_gen,
            e.checkout_key.clone(),
            Ok(ReplyData::File(text_file("orig"))),
        ));
        assert_eq!(e.doc.as_ref().unwrap().reveal_line, Some(3));
        let (token, revision) = issue_reload(&mut e);
        e.apply_reply(reload_reply(
            &e,
            token,
            revision,
            Ok(ReplyData::File(text_file("disk"))),
        ));
        assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "disk");
    }

    #[test]
    fn install_doc_leaving_changes_cancels_acp() {
        let mut e = editor();
        select(&mut e);
        e.section = Section::Changes;
        e.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        e.install_doc("a.txt".into(), text_file("fresh"), None);
        assert_eq!(e.section, Section::Files);
        assert_eq!(e.acp.generating_for, None, "Changes→Files must cancel ACP");
    }

    #[test]
    fn install_doc_same_path_leaving_changes_cancels_acp() {
        let mut e = editor();
        select(&mut e);
        e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
        e.section = Section::Changes;
        e.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
        e.install_doc("a.txt".into(), text_file("orig"), Some(2));
        assert_eq!(e.section, Section::Files);
        assert_eq!(e.doc.as_ref().unwrap().reveal_line, Some(2));
        assert_eq!(e.acp.generating_for, None);
    }

    // -- Backend::shutdown ---------------------------------------------------

    #[test]
    fn shutdown_joins_worker_and_refuses_new_jobs() {
        let mut backend = Backend::new(crate::test_handle());
        // Queue reads + a mutating job, then shut down: the worker must
        // drain mutations, skip reads and exit — join() returning is the
        // proof (a leaked worker would hang the test).
        for _ in 0..8 {
            assert!(backend.send(Job::read(
                "p1",
                PathBuf::from("/tmp/co"),
                "k".into(),
                Req::Changes
            )));
        }
        assert!(backend.send(Job::write(
            "p1",
            PathBuf::from("/tmp/co"),
            "k".into(),
            Req::SetSetting {
                key: "k".into(),
                value: "v".into(),
            },
        )));
        backend.shutdown();
        backend.shutdown(); // idempotent
        assert!(!backend.send(Job::project(Req::Projects)));
    }

    #[test]
    fn shutdown_skips_reads_but_runs_mutations() {
        // The drain predicate is the contract: reads/validation drop,
        // writes finish. Pinned directly so a future Req variant can't
        // silently join the wrong side.
        for req in [
            Req::Changes,
            Req::FileList,
            Req::File {
                path: "a".into(),
                token: 1,
                line: None,
            },
            Req::ReloadFile {
                path: "a".into(),
                token: 1,
                revision: 1,
            },
            Req::Highlight {
                path: "a".into(),
                revision: 1,
                text: String::new(),
            },
            Req::ExternalOpen {
                project_id: "p".into(),
                checkout_path: "c".into(),
                path: "a".into(),
                line: None,
            },
            Req::Projects,
            Req::GetSetting { key: "k".into() },
        ] {
            assert!(!is_mutating(&req), "{req:?} must be skippable on shutdown");
        }
        for req in [
            Req::AddProject("/tmp/x".into()),
            Req::SetSetting {
                key: "k".into(),
                value: "v".into(),
            },
            Req::SaveFile {
                path: "a".into(),
                expected: String::new(),
                text: String::new(),
            },
            Req::Stage { paths: None },
            Req::Unstage { paths: None },
            Req::Discard {
                layer: "l".into(),
                path: "a".into(),
            },
            Req::Commit {
                message: "m".into(),
            },
            Req::Push,
        ] {
            assert!(is_mutating(&req), "{req:?} must finish on shutdown");
        }
    }
}

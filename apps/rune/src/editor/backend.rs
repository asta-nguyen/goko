//! Blocking filesystem/Git work, kept off the UI thread.
//!
//! One worker thread owns a `Storage` connection and a bounded job queue;
//! every job carries the checkout identity it belongs to so a late reply can
//! never land on another checkout's state. Writes ride the same single lane,
//! which serializes them for free. Replies reach the UI through
//! `AppHandle::update`, which also wakes the event loop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};

use app_core::activity::{Activity, SessionActivity};
use app_core::agents::{self, AgentInfo};
use app_core::error::BridgeError;
use app_core::git_view::{
    self, GitViewBranch, GitViewChanges, GitViewDiff, GitViewFile, GitViewHistory, GitViewSearch,
    GitViewStash,
};
use app_core::git_write::{self, GitCommitResult, GitPushResult};
#[cfg(not(test))]
use app_core::home;
use app_core::projects::{self, ProjectView};
use app_core::store::{SessionMeta, Storage};
use app_core::worktrees::{self, WorktreeList};
use rune::AppHandle;

use crate::State;
use crate::editor::state::{EditorState, FileMark, Notice, Section};
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
    /// Register a natively picked folder in the given workspace.
    AddProject {
        path: String,
        workspace_id: String,
    },
    /// Scan a folder for nested Git worktree roots. The token ties the reply
    /// to the dialog that asked; a dismissed scan's result is dropped.
    ScanNestedRepos {
        path: String,
        token: u64,
    },
    /// Register the reviewed repositories of one folder (group, separate or
    /// folder mode) in the selected workspace.
    ImportNestedRepos {
        parent_path: String,
        parent_id: Option<String>,
        group_name: String,
        project_paths: Vec<String>,
        mode: app_core::projects::ImportMode,
        workspace_id: String,
        token: u64,
    },
    /// Every named workspace, Default first.
    Workspaces,
    /// Create a named workspace and return it.
    CreateWorkspace {
        name: String,
        /// Correlates the reply with the prompt whose drafts it may clear.
        token: u64,
    },
    /// Rename a named workspace; the reply carries the refreshed list.
    RenameWorkspace {
        id: String,
        name: String,
        /// Correlates the reply with the prompt whose drafts it may clear.
        token: u64,
    },
    /// Remove a named workspace, moving its projects to Default.
    RemoveWorkspace {
        id: String,
    },
    /// Move one project to another workspace.
    MoveProject {
        project_id: String,
        workspace_id: String,
    },
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
    /// Every stored session metadata row — the app's join table between a
    /// daemon session and the project/agent it belongs to.
    SessionMetaList,
    /// Record why a daemon session exists (its project and agent).
    InsertSessionMeta(SessionMeta),
    /// Drop a session's metadata row (the session was deleted).
    DeleteSessionMeta(String),
    /// The agents a session can be started with, discovered on the worker.
    Agents,
    /// The launch argv for one agent, resolved on the worker: the binary on
    /// `PATH`, the default shell for `shell`, Claude's own hooks — for the
    /// account `accounts::resolve` names, so a named account launches with
    /// its own `CLAUDE_CONFIG_DIR`/`CODEX_HOME`.
    AgentArgv {
        agent: String,
        /// Correlates the reply with the row + agent that asked for it:
        /// two launches can be in flight at once, so the reply's own
        /// token — not the latest pending request — decides which one
        /// starts.
        token: u64,
    },
    /// One usage/accounts read: the snapshot plus the account view the
    /// chips and session labels are drawn from. `previous` keeps the last
    /// values for sources that cannot be read this tick.
    Usage {
        previous: app_core::usage::UsageSnapshot,
    },
    /// Add a named account (metadata + its folder), then activate it.
    AccountAdd {
        agent: String,
        name: String,
        dir: String,
        /// Correlates the reply with the submission whose drafts it may
        /// clear; a stale reply never wipes a newer form.
        token: u64,
    },
    /// Rename a stored account row (metadata only).
    AccountRename {
        agent: String,
        id: String,
        name: String,
        /// Correlates the reply with the inline editor that sent it.
        token: u64,
    },
    /// Remove a stored account row — never its folder or credentials.
    AccountRemove {
        agent: String,
        id: String,
    },
    /// Make `id` — `default` or a stored id — the agent's active account.
    AccountSetActive {
        agent: String,
        id: String,
    },
    /// The agent's own login command for an added account, parsed into a
    /// session-ready argv on the worker.
    AccountSignIn {
        agent: String,
        id: String,
    },
    /// Create a linked worktree for a project.
    CreateWorktree {
        project_id: String,
        path: String,
        mode: String,
        branch: String,
        base: Option<String>,
    },
    /// Local branches (with their checked-out paths) and base refs for the
    /// New-worktree form — `worktrees::branches`.
    WorktreeBranches {
        project_id: String,
    },
    /// The `.env*` files a checkout would offer to copy — the form's copy
    /// preview (`worktrees::env_candidates`).
    EnvCandidates {
        project_id: String,
        source_path: String,
    },
    /// The form's create: `worktrees::create` then, when `env_source` is
    /// set, `worktrees::env_copy` into the new checkout. A copy failure is
    /// carried inside `WorktreeCreated.env` (partial success), not as the
    /// job's error — the worktree exists either way.
    WorktreeCreate {
        project_id: String,
        path: String,
        mode: String,
        branch: String,
        base: Option<String>,
        env_source: Option<String>,
    },
    /// Retry an env copy into an existing checkout (the sidebar notice's
    /// "Retry copy" — `worktrees::env_copy`).
    EnvCopy {
        project_id: String,
        source_path: String,
        target_path: String,
    },
    /// Remove a linked worktree.
    RemoveWorktree {
        project_id: String,
        path: String,
    },
    /// Drop Git's records of worktrees whose folder is gone
    /// (`git worktree prune`; no folder is touched).
    PruneWorktrees {
        project_id: String,
    },
    /// Remove a registered project.
    RemoveProject {
        id: String,
    },
    /// Read the child process's activity: what the session's agent is
    /// doing (working, waiting, done). The worker reads the files the
    /// agent writes; `previous` keeps the last value for a session whose
    /// source cannot be read this tick.
    Activity {
        sessions: Vec<SessionActivity>,
        previous: HashMap<String, Activity>,
    },
    /// The agent CLIs running under the sessions' process trees.
    Subagents {
        sessions: Vec<protocol::session::SessionInfo>,
        inspect: Vec<String>,
    },
    // reads (all take the job's checkout)
    Changes,
    /// the background Changes read: same as `Changes`, applied without a loading indicator
    ChangesPoll,
    CurrentBranch,
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
    /// Ranked Concept search over the derived index — a separate kind so it
    /// never supersedes (or is superseded by) Content search.
    ConceptSearch {
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
    /// The autorefresh probe's quiet re-read of the open doc's file (the
    /// same `git_view::file` read as `File`). The reply only ever adjusts
    /// the doc's disk notice — or a clean buffer, silently — while
    /// `revision` still matches the `CodeState` it was issued at.
    CheckFile {
        path: String,
        revision: u64,
    },
    Stashes,
    StashDiff {
        stash: String,
    },
    /// Per-line blame for the open file: who last touched each line. The
    /// reply is keyed to the path it was asked for, so a reply for a
    /// document that has since been replaced is dropped.
    Blame {
        path: String,
    },
    /// Which lines of the open file the working tree changed against `HEAD`,
    /// for the change gutter. Keyed to its path like `Blame`.
    LineChanges {
        path: String,
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
    /// The "File changed on disk" row's Overwrite: write the buffer over a
    /// disk drift. The worker re-reads the disk file and passes ITS text as
    /// `save_file`'s expected original, so the atomic replace keeps its
    /// read-only/escape checks and still refuses a second drift.
    OverwriteFile {
        path: String,
        text: String,
    },
    /// One level of an ignored directory, read from disk (the file list
    /// leaves it collapsed). Sent on the write lane: its reply must apply
    /// whatever else was clicked since, so it is never generation-gated.
    IgnoredChildren {
        dir: String,
    },
    /// Create an empty file or a folder named `name` in `dir` (`""` = root).
    CreateEntry {
        dir: String,
        name: String,
        folder: bool,
    },
    /// Move a file or folder to the system Trash.
    TrashPath {
        path: String,
    },
    /// Rename a file or folder in place; `new_name` is a plain name.
    RenamePath {
        path: String,
        new_name: String,
    },
    /// Stage a conflicted file after the worker rechecks its text against
    /// `expected`; `allow_markers` is set only by the confirmation dialog,
    /// never by the plain Mark Resolved flow.
    StageResolved {
        path: String,
        expected: String,
        allow_markers: bool,
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
    /// Resolve one conflicted path with a whole-file action (`current`,
    /// `incoming`, `keep`, `delete`); the worker re-reads status and
    /// refuses an action the path's conflict kind does not admit.
    Resolve {
        path: String,
        action: String,
    },
    /// Abort or continue the in-progress merge, rebase, cherry-pick or
    /// revert. Detached like Commit: `--continue` can run the repo's hooks.
    Operation {
        step: String,
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
    NestedScan {
        token: u64,
        result: app_core::project_scan::NestedScanResult,
    },
    NestedImport {
        token: u64,
        result: app_core::projects::NestedImportResult,
    },
    Workspaces(Vec<app_core::store::Workspace>),
    WorkspaceCreated(app_core::store::Workspace),
    Worktrees(WorktreeList),
    Setting(Option<String>),
    /// The stored session metadata rows.
    SessionMetas(Vec<SessionMeta>),
    /// The discovered agents.
    Agents(Vec<AgentInfo>),
    /// The resolved launch argv of one agent.
    AgentArgv(Vec<String>),
    /// The usage snapshot plus the accounts view and the per-provider usage
    /// records read with it.
    Usage {
        snapshot: app_core::usage::UsageSnapshot,
        accounts: app_core::accounts::AccountsView,
        records: Vec<app_core::usage_records::UsageRecord>,
    },
    /// The accounts view after an account mutation.
    Accounts(app_core::accounts::AccountsView),
    /// The sign-in session launch: parsed argv, account folder, title.
    SignIn {
        argv: Vec<String>,
        cwd: String,
        title: String,
    },
    /// The worktree a create request made.
    Worktree(app_core::worktrees::WorktreeInfo),
    /// Branches/bases for the New-worktree form.
    WorktreeBranches(app_core::worktrees::WorktreeBranches),
    /// The env-file preview for one source checkout.
    EnvCandidates(Vec<app_core::worktrees::EnvFileCandidate>),
    /// The form's create result: the new checkout plus the env copy's
    /// outcome when one was asked for (`Err` = partial success — the
    /// worktree exists, the copy failed).
    WorktreeCreated(WorktreeCreated),
    /// A standalone env copy result (the notice's retry).
    EnvCopy(app_core::worktrees::EnvCopyReport),
    /// The child process's activity, keyed by session id.
    Activity(HashMap<String, Activity>),
    /// The agent CLIs under each session, keyed by session id.
    Subagents {
        agents: app_core::subagents::SubagentMap,
        debug: app_core::session_debug::Snapshot,
    },
    Changes(GitViewChanges),
    CurrentBranch(crate::editor::branch::Branch),
    Diff(GitViewDiff),
    History(GitViewHistory),
    Branches(Vec<GitViewBranch>),
    /// Tracked + non-ignored paths, then Git's ignored entries (`dir/` collapsed),
    /// then whether a folder list hit its cap.
    FileList(Vec<String>, Vec<String>, bool),
    Search(GitViewSearch),
    File(GitViewFile),
    Highlight(crate::editor::syntax::HighlightResult),
    Stashes(Vec<GitViewStash>),
    StashDiff(GitViewDiff),
    /// Per-line blame for the file the request named.
    Blame(app_core::git_view::GitViewBlame),
    /// Changed-line spans for the file the request named.
    LineChanges(Vec<app_core::git_view::GitViewLineChange>),
    ExternalOpen {
        checkout: PathBuf,
        path: String,
        line: Option<u64>,
    },
    Saved(GitViewFile),
    /// The entries one level below an ignored directory.
    IgnoredChildren(Vec<String>),
    /// The checkout-relative path of a file or folder just created.
    Created(String),
    /// The new checkout-relative path after a rename.
    Renamed(String),
    /// The config lookup's answer: `paths` aliases parsed worker-side from
    /// the nearest `tsconfig`/`jsconfig` candidate that defined any
    /// (empty when none did).
    ConfigAliases(Vec<crate::editor::go_to_definition::PathAlias>),
    Ok,
    Commit(GitCommitResult),
    Push(GitPushResult),
}

/// `Req::WorktreeCreate`'s reply payload: the created checkout plus the
/// optional env-copy outcome. `env == Some(Err)` is the partial-success
/// case the desktop reports as `envResult.error`.
#[derive(Debug)]
pub struct WorktreeCreated {
    pub info: app_core::worktrees::WorktreeInfo,
    pub env: Option<Result<app_core::worktrees::EnvCopyReport, String>>,
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
        Req::AddProject { .. }
            | Req::ImportNestedRepos { .. }
            | Req::CreateWorkspace { .. }
            | Req::RenameWorkspace { .. }
            | Req::RemoveWorkspace { .. }
            | Req::MoveProject { .. }
            | Req::SetSetting { .. }
            | Req::InsertSessionMeta(_)
            | Req::DeleteSessionMeta(_)
            | Req::AccountAdd { .. }
            | Req::AccountRename { .. }
            | Req::AccountRemove { .. }
            | Req::AccountSetActive { .. }
            | Req::CreateWorktree { .. }
            | Req::WorktreeCreate { .. }
            | Req::EnvCopy { .. }
            | Req::RemoveWorktree { .. }
            | Req::PruneWorktrees { .. }
            | Req::RemoveProject { .. }
            | Req::SaveFile { .. }
            | Req::OverwriteFile { .. }
            | Req::RenamePath { .. }
            | Req::TrashPath { .. }
            | Req::CreateEntry { .. }
            | Req::StageResolved { .. }
            | Req::Stage { .. }
            | Req::Unstage { .. }
            | Req::Discard { .. }
            | Req::Resolve { .. }
            | Req::Operation { .. }
            | Req::Commit { .. }
            | Req::Push
    )
}

/// Test-only backend whose job stream the caller drains itself: returns the
/// `Backend` plus the receiving end, so tests can run each job through
/// `run_job` against a temp `Storage` and apply replies in order — the same
/// FIFO the worker thread produces, without threads or a real home.
#[cfg(test)]
pub(crate) fn channel_backend() -> (Backend, std::sync::mpsc::Receiver<Job>) {
    let (tx, rx) = sync_channel(PENDING_CAP);
    (
        Backend {
            tx: Some(tx),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
            overloaded: false,
        },
        rx,
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
    let ask_handle = handle.clone();
    let ask = move |request: crate::askpass::AskpassRequest| {
        ask_handle.update(move |s: &mut State, _tx| {
            s.askpass = Some(crate::askpass::AskpassState::new(request));
        });
    };
    serve(
        rx,
        &storage,
        &shutdown,
        move |reply| {
            let h = handle.clone();
            handle.update(move |s: &mut State, _tx| {
                s.apply_reply(reply);
                crate::editor::worktree_form::after_reply(s);
                // The Changes section keeps its patch-pane selection valid —
                // the desktop's "select first change" effect, run where a
                // `&mut State` exists (replies land outside the render pass).
                if s.editor.section == Section::Changes {
                    crate::editor::changes::ensure_selection(s);
                }
                let _ = &h;
            });
        },
        ask,
    );
}

/// How often the worker looks for a finished detached job while it waits for
/// the next queued one.
const DETACHED_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Git writes that last as long as a network or a hook does: `git push` (up
/// to its two-minute limit), `git commit` and merge/rebase/cherry-pick/
/// revert operations (the repository's hooks). They run on a thread of
/// their own so every other job keeps its turn.
fn detachable(req: &Req) -> bool {
    matches!(req, Req::Push | Req::Commit { .. } | Req::Operation { .. })
}

/// Writes to a checkout's index and refs. They keep their queue order behind
/// a detached one: a stage must not land in the middle of a commit, and Git
/// would refuse it over `index.lock` anyway.
fn is_git_write(req: &Req) -> bool {
    matches!(
        req,
        Req::Stage { .. }
            | Req::Unstage { .. }
            | Req::Discard { .. }
            | Req::StageResolved { .. }
            | Req::Resolve { .. }
            | Req::Operation { .. }
            | Req::Commit { .. }
            | Req::Push
    )
}

/// Run the queue in order and hand each reply to `post`.
///
/// Everything runs on this thread, one job at a time, except the
/// [`detachable`] git writes. While one of those runs for a checkout, the
/// later [`is_git_write`] jobs for the same checkout wait behind it in order;
/// every other job (file opens and saves, highlighting, reads, the polls,
/// other checkouts) carries on.
///
/// Once the queue closes, the detached jobs and the writes behind them still
/// finish, as every accepted mutating job does.
fn serve(
    rx: std::sync::mpsc::Receiver<Job>,
    storage: &Option<Storage>,
    shutdown: &AtomicBool,
    post: impl Fn(Reply) + Clone + Send + 'static,
    ask: impl Fn(crate::askpass::AskpassRequest) + Clone + Send + 'static,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let (done_tx, done_rx) = std::sync::mpsc::channel::<String>();
    // Checkout key -> the git writes waiting behind its detached job.
    let mut busy: HashMap<String, std::collections::VecDeque<Job>> = HashMap::new();
    let mut detach = |job: Job| detach_job(job, storage, &done_tx, &post, &ask);
    loop {
        while let Ok(key) = done_rx.try_recv() {
            release(&key, &mut busy, storage, &post, &mut detach);
        }
        let job = if busy.is_empty() {
            match rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        } else {
            match rx.recv_timeout(DETACHED_POLL) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        if shutdown.load(Ordering::SeqCst) && !is_mutating(&job.req) {
            // Shutdown drains the queue but never starts new reads — a
            // 128-job backlog must not delay exit behind git calls. The
            // reply still posts an Err so any still-mounted UI unwinds
            // its loading flags deterministically.
            post(Reply {
                generation: job.generation,
                req: job.req.clone(),
                checkout_key: job.checkout_key.clone(),
                result: Err("editor backend is shutting down".to_string()),
            });
            continue;
        }
        // A scan walks the filesystem: run it on its own thread so file
        // opens and saves keep their turn. It reads no store.
        if matches!(job.req, Req::ScanNestedRepos { .. }) {
            let inline = job.clone();
            let post_thread = post.clone();
            let spawned = std::thread::Builder::new()
                .name("goko-project-scan".into())
                .spawn(move || post_thread(run_job(&None, &job)));
            if spawned.is_err() {
                post(run_job(storage, &inline));
            }
            continue;
        }
        let key = job.checkout_key.clone().filter(|_| is_git_write(&job.req));
        let Some(key) = key else {
            post(run_job(storage, &job));
            continue;
        };
        if let Some(waiting) = busy.get_mut(&key) {
            waiting.push_back(job);
        } else if detachable(&job.req) {
            busy.insert(key.clone(), std::collections::VecDeque::new());
            if !detach(job) {
                release(&key, &mut busy, storage, &post, &mut detach);
            }
        } else {
            post(run_job(storage, &job));
        }
    }
    while !busy.is_empty() {
        let Ok(key) = done_rx.recv() else { break };
        release(&key, &mut busy, storage, &post, &mut detach);
    }
}

/// The detached job of `key` finished: run the writes that waited behind it,
/// in order, until the queue is empty or one of them is detached in turn.
fn release(
    key: &str,
    busy: &mut HashMap<String, std::collections::VecDeque<Job>>,
    storage: &Option<Storage>,
    post: &impl Fn(Reply),
    detach: &mut impl FnMut(Job) -> bool,
) {
    loop {
        let Some(next) = busy.get_mut(key).and_then(|waiting| waiting.pop_front()) else {
            busy.remove(key);
            return;
        };
        if !detachable(&next.req) {
            post(run_job(storage, &next));
        } else if detach(next) {
            return;
        }
    }
}

/// Start a [`detachable`] job on its own thread. The checkout is validated
/// here, on the worker and against its store, exactly as `run_checkout` does;
/// only the git command itself runs detached. Returns `false` when nothing
/// was started — the reply has been posted already.
fn detach_job(
    job: Job,
    storage: &Option<Storage>,
    done: &std::sync::mpsc::Sender<String>,
    post: &(impl Fn(Reply) + Clone + Send + 'static),
    ask: &(impl Fn(crate::askpass::AskpassRequest) + Clone + Send + 'static),
) -> bool {
    let validated = match (&job.project_id, &job.checkout, &job.checkout_key) {
        (Some(project_id), Some(checkout), Some(key)) => storage_req(storage, |store| {
            projects::git_checkout(store, project_id, &checkout.display().to_string())
        })
        .map(|checkout| (checkout, key.clone())),
        _ => Err("checkout job missing project or path identity".into()),
    };
    let (checkout, key) = match validated {
        Ok(validated) => validated,
        Err(error) => {
            post(Reply {
                generation: job.generation,
                req: job.req.clone(),
                checkout_key: job.checkout_key.clone(),
                result: Err(error),
            });
            return false;
        }
    };
    let (done, thread_post, thread_job) = (done.clone(), post.clone(), job.clone());
    let thread_ask = (*ask).clone();
    let spawned = std::thread::Builder::new()
        .name("goko-editor-git".into())
        .spawn(move || {
            let job = thread_job;
            let result = match &job.req {
                Req::Push => match crate::askpass::Session::start(thread_ask) {
                    Ok(session) => {
                        let envs = session.envs();
                        let result = git_write::push_with_env(&checkout, &envs);
                        let cancelled = session.cancelled();
                        drop(session);
                        match result {
                            Err(_) if cancelled => Err("Push cancelled".to_string()),
                            other => other.map(ReplyData::Push).map_err(|e| e.to_string()),
                        }
                    }
                    Err(_) => git_write::push(&checkout)
                        .map(ReplyData::Push)
                        .map_err(|e| e.to_string()),
                },
                Req::Commit { message } => git_write::commit(&checkout, message)
                    .map(ReplyData::Commit)
                    .map_err(|e| e.to_string()),
                Req::Operation { step } => git_write::operation(&checkout, step)
                    .map(|_| ReplyData::Ok)
                    .map_err(|e| e.to_string()),
                _ => Err("not a detachable job".to_string()),
            };
            thread_post(Reply {
                generation: job.generation,
                req: job.req,
                checkout_key: job.checkout_key,
                result,
            });
            let _ = done.send(key);
        });
    if spawned.is_err() {
        // No thread to spare: run it here, as it used to run.
        post(run_job(storage, &job));
        return false;
    }
    true
}

pub(crate) fn run_job(storage: &Option<Storage>, job: &Job) -> Reply {
    let checkout_key = job.checkout_key.clone();
    let result = match &job.req {
        Req::Projects => storage_req(storage, |store| {
            projects::list_projects(store).map(ReplyData::Projects)
        }),
        Req::AddProject { path, workspace_id } => storage_req(storage, |store| {
            projects::add_project(store, path, workspace_id).map(ReplyData::Added)
        }),
        Req::ScanNestedRepos { path, token } => app_core::project_scan::scan(
            std::path::Path::new(path),
            app_core::project_scan::ScanOptions::default(),
        )
        .map(|result| ReplyData::NestedScan {
            token: *token,
            result,
        })
        .map_err(|error| error.to_string()),
        Req::ImportNestedRepos {
            parent_path,
            parent_id,
            group_name,
            project_paths,
            mode,
            workspace_id,
            token,
        } => storage_req(storage, |store| {
            projects::import_nested(
                store,
                parent_path,
                parent_id.as_deref(),
                group_name,
                project_paths,
                *mode,
                workspace_id,
            )
            .map(|result| ReplyData::NestedImport {
                token: *token,
                result,
            })
        }),
        Req::Workspaces => storage_req(storage, |store| {
            store.list_workspaces().map(ReplyData::Workspaces)
        }),
        Req::CreateWorkspace { name, .. } => storage_req(storage, |store| {
            store
                .create_workspace(name)
                .map(ReplyData::WorkspaceCreated)
        }),
        Req::RenameWorkspace { id, name, .. } => storage_req(storage, |store| {
            store.rename_workspace(id, name)?;
            store.list_workspaces().map(ReplyData::Workspaces)
        }),
        Req::RemoveWorkspace { id } => storage_req(storage, |store| {
            store.remove_workspace(id)?;
            store.list_workspaces().map(ReplyData::Workspaces)
        }),
        Req::MoveProject {
            project_id,
            workspace_id,
        } => storage_req(storage, |store| {
            store
                .move_project(project_id, workspace_id)
                .map(|_| ReplyData::Ok)
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
        Req::SessionMetaList => storage_req(storage, |store| {
            store.list_session_meta().map(ReplyData::SessionMetas)
        }),
        Req::InsertSessionMeta(meta) => storage_req(storage, |store| {
            store.insert_session_meta(meta).map(|_| ReplyData::Ok)
        }),
        Req::DeleteSessionMeta(id) => storage_req(storage, |store| {
            store.delete_session_meta(id).map(|_| ReplyData::Ok)
        }),
        Req::Agents => Ok(ReplyData::Agents(agents::discover(
            std::env::var_os("PATH").as_deref(),
            &default_shell(),
        ))),
        Req::AgentArgv { agent, .. } => agent_argv(storage, agent),
        Req::Usage { previous } => usage_snapshot(storage, previous),
        Req::AccountAdd {
            agent, name, dir, ..
        } => storage_req(storage, |store| {
            let default_dir = app_core::accounts::default_dir(agent);
            let accounts = app_core::accounts::add(
                store,
                agent,
                name,
                dir,
                default_dir.as_deref(),
                std::env::var_os("HOME").as_deref(),
            )?;
            Ok(ReplyData::Accounts(app_core::accounts::view(&accounts)))
        }),
        Req::AccountRename {
            agent, id, name, ..
        } => storage_req(storage, |store| {
            let accounts = app_core::accounts::rename(store, agent, id, name)?;
            Ok(ReplyData::Accounts(app_core::accounts::view(&accounts)))
        }),
        Req::AccountRemove { agent, id } => storage_req(storage, |store| {
            let accounts = app_core::accounts::remove(store, agent, id)?;
            Ok(ReplyData::Accounts(app_core::accounts::view(&accounts)))
        }),
        Req::AccountSetActive { agent, id } => storage_req(storage, |store| {
            let accounts = app_core::accounts::set_active(store, agent, id)?;
            Ok(ReplyData::Accounts(app_core::accounts::view(&accounts)))
        }),
        Req::AccountSignIn { agent, id } => {
            let Some(storage) = storage else {
                return Reply {
                    generation: job.generation,
                    req: job.req.clone(),
                    checkout_key,
                    result: Err("storage is unavailable".into()),
                };
            };
            let accounts = app_core::accounts::read(storage);
            let result = app_core::accounts::sign_in_launch(
                &accounts,
                agent,
                id,
                std::env::var_os("PATH").as_deref(),
            )
            .and_then(|launch| {
                app_core::agents::shell_words(&launch.command)
                    .map(|argv| (argv, launch.cwd, launch.title))
                    .ok_or_else(|| {
                        BridgeError::new(
                            app_core::error::Code::Daemon(
                                protocol::message::ErrorCode::InvalidArgument,
                            ),
                            "the sign-in command cannot be parsed",
                        )
                    })
            })
            .map(|(argv, cwd, title)| ReplyData::SignIn { argv, cwd, title })
            .map_err(|error| error.to_string());
            return Reply {
                generation: job.generation,
                req: job.req.clone(),
                checkout_key,
                result,
            };
        }
        Req::CreateWorktree {
            project_id,
            path,
            mode,
            branch,
            base,
        } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::create(
                std::path::Path::new(&root),
                path,
                mode,
                branch,
                base.as_deref(),
            )
            .map(ReplyData::Worktree)
        }),
        Req::WorktreeBranches { project_id } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::branches(std::path::Path::new(&root)).map(ReplyData::WorktreeBranches)
        }),
        Req::EnvCandidates {
            project_id,
            source_path,
        } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::env_candidates(std::path::Path::new(&root), source_path)
                .map(ReplyData::EnvCandidates)
        }),
        Req::WorktreeCreate {
            project_id,
            path,
            mode,
            branch,
            base,
            env_source,
        } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            let root = std::path::Path::new(&root);
            let info = worktrees::create(root, path, mode, branch, base.as_deref())?;
            // Partial success: the checkout exists even when the copy
            // fails — carry the copy's outcome, not a job-level error.
            let env = env_source.as_ref().map(|source| {
                worktrees::env_copy(root, source, &info.path).map_err(|e| e.to_string())
            });
            Ok(ReplyData::WorktreeCreated(WorktreeCreated { info, env }))
        }),
        Req::EnvCopy {
            project_id,
            source_path,
            target_path,
        } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::env_copy(std::path::Path::new(&root), source_path, target_path)
                .map(ReplyData::EnvCopy)
        }),
        Req::RemoveWorktree { project_id, path } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            // A dirty checkout would refuse the removal; the user asked for
            // it explicitly, so the force flag is the caller's choice.
            worktrees::remove(std::path::Path::new(&root), path, true).map(|_| ReplyData::Ok)
        }),
        Req::PruneWorktrees { project_id } => storage_req(storage, |store| {
            let root = projects::project_root(store, project_id)?;
            worktrees::prune(std::path::Path::new(&root)).map(|_| ReplyData::Ok)
        }),
        Req::RemoveProject { id } => storage_req(storage, |store| {
            projects::remove_project(store, id).map(|_| ReplyData::Ok)
        }),
        Req::Subagents { sessions, inspect } => {
            // The process table is read on the worker: `ps` is a command,
            // not a file the UI thread should run.
            let (rows, rss) = process_table();
            let mut map = app_core::subagents::compute_subagents(sessions, &rows);
            // Codex 0.167 runs its agents under a shared `codex app-server`
            // daemon: those are attributed through the thread id.
            add_codex_daemon_agents(&mut map, sessions, &rows);
            let debug = app_core::session_debug::sample(sessions, &rows, rss, &map, inspect);
            Ok(ReplyData::Subagents { agents: map, debug })
        }
        Req::Activity { sessions, previous } => {
            // The files live in GOKO_HOME and the codex home; reading them
            // is the worker's job.
            let goko_home = agent_home();
            let codex_home = app_core::activity::codex_home_from(
                std::env::var("CODEX_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
            );
            Ok(ReplyData::Activity(app_core::activity::compute_activity(
                sessions,
                &goko_home,
                codex_home.as_deref(),
                previous,
            )))
        }
        Req::ExternalOpen {
            project_id,
            checkout_path,
            path,
            line,
        } => storage_req(storage, |store| {
            // Same validation every file job gets, plus the once-only open
            // payload — wrong project/path/checkouts fail here.
            let (checkout, _) = projects::file_checkout(store, project_id, checkout_path)?;
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

/// The agents the shared `codex app-server` daemon runs for a session:
/// they are attributed through the `CODEX_THREAD_ID` their tool calls
/// carry, matched to the session by its rollout.
fn add_codex_daemon_agents(
    map: &mut app_core::subagents::SubagentMap,
    sessions: &[protocol::session::SessionInfo],
    rows: &[app_core::subagents::ProcessRow],
) {
    let codex_home = app_core::activity::codex_home_from(
        std::env::var("CODEX_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    );
    let Some(codex_home) = codex_home else {
        return;
    };
    let agents = app_core::subagents::codex_daemon_agents(rows, map);
    if agents.is_empty() {
        return;
    }
    let pids: Vec<u32> = agents.iter().map(|row| row.pid).collect();
    let threads = thread_ids(&pids);
    // Back to the day the oldest Codex session started: a session that has
    // run for days still owns the rollout in its first day's directory.
    let since = sessions
        .iter()
        .filter(|session| app_core::activity::is_codex(&session.argv))
        .map(|session| session.started_at)
        .min();
    let owners = app_core::subagents::thread_owners(
        sessions,
        &app_core::activity::rollouts_since(&codex_home, since),
    );
    app_core::subagents::attribute_codex(map, &agents, &threads, &owners);
}

/// pid -> `CODEX_THREAD_ID`, read from the processes' environments. Linux
/// serves each one as a file; `ps` there has no option that prints it the way
/// macOS's `-E` does.
#[cfg(target_os = "linux")]
fn thread_ids(pids: &[u32]) -> HashMap<u32, String> {
    pids.iter()
        .filter_map(|pid| {
            let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
            Some((*pid, app_core::subagents::thread_id_in_environ(&environ)?))
        })
        .collect()
}

/// pid -> `CODEX_THREAD_ID`. Windows has no `ps -E`: attribution degrades to
/// no thread ids (documented v1).
#[cfg(windows)]
fn thread_ids(_pids: &[u32]) -> HashMap<u32, String> {
    HashMap::new()
}

/// pid -> `CODEX_THREAD_ID`, read from the processes' environments.
#[cfg(all(not(target_os = "linux"), not(windows)))]
fn thread_ids(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let joined = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("ps")
        .args(["-Eww", "-o", "pid=,command=", "-p", joined.as_str()])
        .output()
    else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    app_core::subagents::parse_thread_ids(&String::from_utf8_lossy(&output.stdout))
}

/// The process table: `pid`, `ppid` and the command of every process.
/// Windows has no `ps`: attribution degrades to no rows (documented v1).
#[cfg(windows)]
fn process_table() -> (Vec<app_core::subagents::ProcessRow>, HashMap<u32, u64>) {
    Default::default()
}

/// The process table: `pid`, `ppid` and the command of every process.
#[cfg(not(windows))]
fn process_table() -> (Vec<app_core::subagents::ProcessRow>, HashMap<u32, u64>) {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,rss=,command="])
        .output()
    else {
        return Default::default();
    };
    if !output.status.success() {
        return Default::default();
    }
    app_core::session_debug::parse_processes(&String::from_utf8_lossy(&output.stdout))
}

/// The shell a `shell` session runs: `$SHELL`, else `/bin/zsh` — the same
/// rule the desktop's `default_shell` uses.
/// The user's default shell: `$SHELL`, else `/bin/zsh` on Unix and
/// `%COMSPEC%` (or `powershell.exe`) on Windows.
pub fn default_shell() -> String {
    let (variable, fallback) = if cfg!(windows) {
        ("COMSPEC", "powershell.exe")
    } else {
        ("SHELL", "/bin/zsh")
    };
    std::env::var(variable)
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Where Claude's usage and activity hooks write. Tests use a temp
/// directory so no real `GOKO_HOME` is touched.
fn agent_home() -> PathBuf {
    #[cfg(not(test))]
    {
        home::home_dir().unwrap_or_default()
    }
    #[cfg(test)]
    {
        std::env::temp_dir().join("goko-rune-agent-home")
    }
}

/// The launch argv for `agent`, resolved the way the desktop resolves it:
/// the binary on `PATH`, the default shell for `shell`, and Claude's own
/// statusline/hooks for `claude` — under the account `accounts::resolve`
/// names, so a named account carries its `CLAUDE_CONFIG_DIR`/`CODEX_HOME`
/// and Claude captures usage under its id. A named account whose folder
/// vanished is refused, and the original statusline is read from the
/// account's own settings, never always Default's.
fn agent_argv(storage: &Option<Storage>, agent: &str) -> Result<ReplyData, String> {
    // Only Claude and Codex have stored accounts; every other adapter
    // launches as Default. Asking `accounts::resolve` for e.g. `shell`
    // would reject a perfectly valid preset.
    let account = match (agent, storage.as_ref()) {
        ("claude" | "codex", Some(storage)) => {
            app_core::accounts::resolve(agent, storage).map_err(|error| error.to_string())?
        }
        _ => app_core::accounts::LaunchAccount::default(),
    };
    if let Some(dir) = &account.dir
        && !dir.is_dir()
    {
        return Err(format!(
            "the account folder {} no longer exists",
            dir.display()
        ));
    }
    let (goko_home, original_statusline) = if agent == "claude" {
        let config_dir = account
            .dir
            .clone()
            .or_else(|| app_core::accounts::default_dir("claude"));
        let original =
            config_dir.and_then(|dir| agents::original_statusline(&dir.join("settings.json")));
        (agent_home(), original)
    } else {
        (PathBuf::new(), None)
    };
    agents::argv_for_account(
        agent,
        std::env::var_os("PATH").as_deref(),
        &default_shell(),
        &goko_home,
        original_statusline.as_deref(),
        &account,
    )
    .map(ReplyData::AgentArgv)
    .map_err(|error| error.to_string())
}

/// One usage/accounts read for the quota chips and the per-session account
/// labels: the stored accounts (infallible — a broken store reads as
/// Default-only), the Default folders resolved from the environment, and
/// the snapshot folded over `previous` so a mid-write capture keeps its
/// last values.
fn usage_snapshot(
    storage: &Option<Storage>,
    previous: &app_core::usage::UsageSnapshot,
) -> Result<ReplyData, String> {
    let accounts = storage
        .as_ref()
        .map(app_core::accounts::read)
        .unwrap_or_default();
    let goko_home = agent_home();
    let codex_home = app_core::usage::codex_home_from(
        std::env::var_os("CODEX_HOME"),
        app_core::home::user_home(),
    );
    let snapshot =
        app_core::usage::read_snapshot(&goko_home, codex_home.as_deref(), &accounts, previous);
    Ok(ReplyData::Usage {
        snapshot,
        accounts: app_core::accounts::view(&accounts),
        records: app_core::usage_records::read(
            &usage_record_dirs(&goko_home),
            crate::data::now_ms(),
        ),
    })
}

/// The folders usage records are read from. Tests read only the temp
/// `GOKO_HOME`, never the developer's own state folder.
fn usage_record_dirs(goko_home: &std::path::Path) -> Vec<PathBuf> {
    #[cfg(not(test))]
    {
        app_core::usage_records::record_dirs(
            goko_home,
            std::env::var_os("XDG_STATE_HOME"),
            app_core::home::user_home(),
        )
    }
    #[cfg(test)]
    {
        app_core::usage_records::record_dirs(goko_home, None, None)
    }
}

/// Recheck the saved snapshot on the worker before resolving the index
/// entry. Leftover conflict markers are refused unless the request carries
/// the dialog's `allow_markers` confirmation.
fn stage_resolved(
    checkout: &Path,
    path: &str,
    expected: &str,
    allow_markers: bool,
) -> Result<(), String> {
    let file = git_view::file(checkout, path).map_err(|e| e.to_string())?;
    if file.kind != "text" || file.truncated || file.text.as_deref() != Some(expected) {
        return Err("the file changed on disk; reload it before marking resolved".into());
    }
    if !allow_markers && crate::editor::conflict::has_markers(expected) {
        return Err("resolve all conflict markers before staging".into());
    }
    git_write::stage(checkout, Some(&[path.to_string()])).map_err(|e| e.to_string())
}

/// The Overwrite button's write: re-read the disk file so `save_file`'s
/// expected-text guard compares the buffer against what is really there —
/// a second drift between this read and the replace still refuses, and
/// anything that is no longer a complete text file refuses up front.
fn overwrite_file(checkout: &Path, path: &str, text: &str) -> Result<GitViewFile, String> {
    let disk = git_view::file(checkout, path).map_err(|e| e.to_string())?;
    let expected = match (disk.kind, disk.truncated, disk.text) {
        ("text", false, Some(text)) => text,
        _ => return Err("the file is no longer text".into()),
    };
    git_view::save_file(checkout, path, &expected, text).map_err(|e| e.to_string())
}

/// Requests that need a real Git worktree. Every other checkout-scoped
/// request is a file job and accepts a folder project's root.
fn is_git_request(req: &Req) -> bool {
    matches!(
        req,
        Req::CurrentBranch
            | Req::Changes
            | Req::ChangesPoll
            | Req::Diff { .. }
            | Req::History { .. }
            | Req::Branches
            | Req::Search { .. }
            | Req::GoToSearch { .. }
            | Req::ConceptSearch { .. }
            | Req::Stashes
            | Req::StashDiff { .. }
            | Req::Blame { .. }
            | Req::LineChanges { .. }
            | Req::StageResolved { .. }
            | Req::Resolve { .. }
            | Req::Operation { .. }
            | Req::Stage { .. }
            | Req::Unstage { .. }
            | Req::Discard { .. }
            | Req::Commit { .. }
            | Req::Push
    )
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
    let checkout_path = checkout.display().to_string();
    // File jobs accept a folder project's root; Git jobs keep the worktree
    // requirement. Both revalidate the registered project and path.
    let (checkout, is_git) = if is_git_request(req) {
        let checkout = storage_req(storage, |store| {
            projects::git_checkout(store, project_id, &checkout_path)
        })?;
        (checkout, true)
    } else {
        storage_req(storage, |store| {
            projects::file_checkout(store, project_id, &checkout_path)
        })?
    };
    let checkout = &checkout;
    match req {
        Req::CurrentBranch => crate::editor::branch::read(checkout).map(ReplyData::CurrentBranch),
        Req::Changes | Req::ChangesPoll => git_view::changes(checkout)
            .map(ReplyData::Changes)
            .map_err(|e| e.to_string()),
        Req::Diff {
            layer,
            path,
            commit,
        } => git_view::diff(checkout, layer, path.as_deref(), commit.as_deref())
            .map(ReplyData::Diff)
            .map_err(|e| e.to_string()),
        Req::File { path, .. } | Req::ReloadFile { path, .. } | Req::CheckFile { path, .. } => {
            git_view::file(checkout, path)
                .map(ReplyData::File)
                .map_err(|e| e.to_string())
        }
        Req::History { skip, branch } => git_view::history(checkout, *skip, branch.as_deref())
            .map(ReplyData::History)
            .map_err(|e| e.to_string()),
        Req::Branches => git_view::branches(checkout)
            .map(ReplyData::Branches)
            .map_err(|e| e.to_string()),
        Req::FileList => {
            if is_git {
                git_view::file_list(checkout)
                    .map(|paths| {
                        // The ignored entries only dim extra rows: a failure shows none.
                        let ignored = git_view::ignored_list(checkout).unwrap_or_default();
                        ReplyData::FileList(paths, ignored, false)
                    })
                    .map_err(|e| e.to_string())
            } else {
                git_view::folder_file_list(checkout)
                    .map(|(paths, truncated)| ReplyData::FileList(paths, Vec::new(), truncated))
                    .map_err(|e| e.to_string())
            }
        }
        Req::Search { query } | Req::GoToSearch { query } => git_view::search(checkout, query)
            .map(ReplyData::Search)
            .map_err(|e| e.to_string()),
        Req::ConceptSearch { query } => {
            let Some(storage) = storage.as_ref() else {
                return Err("storage is unavailable".into());
            };
            app_core::code_index::search(storage.home(), checkout, query)
                .map(ReplyData::Search)
                .map_err(|e| e.to_string())
        }
        Req::ConfigFiles { paths } => Ok(config_aliases(checkout, paths)),
        Req::Stashes => git_view::stashes(checkout)
            .map(ReplyData::Stashes)
            .map_err(|e| e.to_string()),
        Req::StashDiff { stash } => git_view::stash_diff(checkout, stash)
            .map(ReplyData::StashDiff)
            .map_err(|e| e.to_string()),
        Req::Blame { path } => git_view::blame(checkout, path)
            .map(ReplyData::Blame)
            .map_err(|e| e.to_string()),
        Req::LineChanges { path } => git_view::line_changes(checkout, path)
            .map(ReplyData::LineChanges)
            .map_err(|e| e.to_string()),
        Req::SaveFile {
            path,
            expected,
            text,
        } => git_view::save_file(checkout, path, expected, text)
            .map(ReplyData::Saved)
            .map_err(|e| e.to_string()),
        Req::OverwriteFile { path, text } => {
            overwrite_file(checkout, path, text).map(ReplyData::Saved)
        }
        Req::IgnoredChildren { dir } => git_view::ignored_children(checkout, dir)
            .map(ReplyData::IgnoredChildren)
            .map_err(|e| e.to_string()),
        Req::CreateEntry { dir, name, folder } => {
            git_view::create_entry(checkout, dir, name, *folder)
                .map(ReplyData::Created)
                .map_err(|e| e.to_string())
        }
        Req::TrashPath { path } => git_view::entry_path(checkout, path)
            .map_err(|e| e.to_string())
            .and_then(|entry| crate::editor::trash::move_to_trash(&entry))
            .map(|_| ReplyData::Ok),
        Req::RenamePath { path, new_name } => git_view::rename_path(checkout, path, new_name)
            .map(ReplyData::Renamed)
            .map_err(|e| e.to_string()),
        Req::StageResolved {
            path,
            expected,
            allow_markers,
        } => stage_resolved(checkout, path, expected, *allow_markers).map(|_| ReplyData::Ok),
        Req::Resolve { path, action } => git_write::resolve(checkout, path, action)
            .map(|_| ReplyData::Ok)
            .map_err(|e| e.to_string()),
        Req::Operation { step } => git_write::operation(checkout, step)
            .map(|_| ReplyData::Ok)
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
        | Req::AddProject { .. }
        | Req::ScanNestedRepos { .. }
        | Req::ImportNestedRepos { .. }
        | Req::Workspaces
        | Req::CreateWorkspace { .. }
        | Req::RenameWorkspace { .. }
        | Req::RemoveWorkspace { .. }
        | Req::MoveProject { .. }
        | Req::Worktrees(_)
        | Req::GetSetting { .. }
        | Req::SetSetting { .. }
        | Req::SessionMetaList
        | Req::InsertSessionMeta(_)
        | Req::DeleteSessionMeta(_)
        | Req::Agents
        | Req::AgentArgv { .. }
        | Req::Usage { .. }
        | Req::AccountAdd { .. }
        | Req::AccountRename { .. }
        | Req::AccountRemove { .. }
        | Req::AccountSetActive { .. }
        | Req::AccountSignIn { .. }
        | Req::CreateWorktree { .. }
        | Req::WorktreeBranches { .. }
        | Req::EnvCandidates { .. }
        | Req::WorktreeCreate { .. }
        | Req::EnvCopy { .. }
        | Req::RemoveWorktree { .. }
        | Req::PruneWorktrees { .. }
        | Req::RemoveProject { .. }
        | Req::Activity { .. }
        | Req::Subagents { .. }
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
        let keep = |p: &str| {
            filter.is_empty()
                || p.rsplit('/')
                    .next()
                    .unwrap_or(p)
                    .to_lowercase()
                    .contains(&filter)
                || p.to_lowercase().contains(&filter)
        };
        let tracked = self
            .files
            .paths
            .iter()
            .map(String::as_str)
            .filter(|p| keep(p))
            .map(|p| (p, FileMark::Tracked));
        // Ignored entries sit beside the tracked ones, dimmed. An ignored
        // directory is a leaf row until the user opens it; then its entries
        // stand in for it.
        let ignored_entries = expand_ignored(&self.files.ignored, &self.files.ignored_open);
        let ignored = ignored_entries
            .iter()
            .filter(|p| keep(p.trim_end_matches('/')))
            .map(|p| match p.strip_suffix('/') {
                Some(dir) => (dir, FileMark::IgnoredDir),
                None => (p.as_str(), FileMark::Ignored),
            });
        // A rebuild (refresh, rename, an opened ignored directory) keeps
        // the folders the user had open and the cursor.
        // A folder just created has no file for Git to list: it stands in
        // as an empty folder row until something lands in it.
        let empty = self
            .files
            .empty_dirs
            .iter()
            .filter(|d| keep(d))
            .filter(|d| {
                let inside = format!("{d}/");
                !self.files.paths.iter().any(|p| p.starts_with(&inside))
                    && !ignored_entries.iter().any(|p| p.starts_with(&inside))
            })
            .map(|d| (d.as_str(), FileMark::EmptyDir));
        let open = expanded_ids(&self.files.model);
        let cursor = self.files.model.cursor().map(str::to_string);
        let mut model = rune_kit::FileTree::new()
            .dirs_first(true)
            .build(tracked.chain(ignored).chain(empty));
        // Folders first — including an ignored or empty folder, which is a
        // leaf row the kit would sort among the files, only to jump up
        // among the folders once opened.
        let folder = |n: &rune_kit::FileNode<FileMark>| {
            n.dir || matches!(n.data, Some(FileMark::IgnoredDir | FileMark::EmptyDir))
        };
        model.sort_by(|a, b| {
            folder(b)
                .cmp(&folder(a))
                .then_with(|| rune_kit::natural_cmp(&a.name, &b.name))
        });
        // The kit merges a folder whose only child is a folder into one row
        // (`public` + `assets` → `public/assets`), so a row's id can change
        // between rebuilds: a folder stays open when its own path, or a
        // folder it merged, was open.
        let open_paths: std::collections::HashSet<&str> = open
            .iter()
            .flat_map(|id| {
                id.match_indices('/')
                    .map(|(i, _)| &id[..i])
                    .chain([id.as_str()])
            })
            .collect();
        for id in folder_ids(&model) {
            if open_paths.contains(id.as_str()) {
                model.set_expanded(&id, true);
            }
        }
        if let Some(id) = cursor.filter(|id| model.contains(id)) {
            model.select(&id);
        }
        self.files.model = model;
        self.files.model_checkout = self.checkout_key.clone();
    }

    /// Files became the active surface: any in-flight ACP generate belonged
    /// to the Changes commit panel, so cancel it with the CURRENT (still
    /// pre-transition) project/checkout identity before the section flips.
    /// Then kick the section's reads — a file opened from Changes or
    /// History lands here without a file list, and `refresh_active` only
    /// enqueues `Req::FileList` when the cached list is missing or belongs
    /// to another checkout.
    fn activate_files(&mut self) {
        if self.section == Section::Changes {
            self.acp.cancel(&self.project_id, &self.checkout);
        }
        self.section = Section::Files;
        self.visited.insert(Section::Files);
        self.refresh_active();
    }

    fn install_doc(&mut self, path: String, file: GitViewFile, line: Option<u64>) {
        // Same file already open → just queue the reveal, no reload
        // (external open-file contract). The doc is untouched, so a pending
        // reload stays valid. A reply for a path that is already a parked
        // tab is dropped the same way — the parked buffer wins over a
        // second copy of the disk file.
        if self.doc.as_ref().is_some_and(|d| d.path == path) || self.tabs.is_open(&path) {
            if let Some(line) = line {
                if let Some(doc) = self.doc.as_mut().filter(|d| d.path == path) {
                    doc.reveal_line = Some(line);
                } else if let Some(doc) = self.tabs.parked.get_mut(&path) {
                    doc.reveal_line = Some(line);
                }
            }
            // A write owns the active slot until its reply lands: the tab
            // is already open, so the reveal just waits with it.
            if !self
                .doc
                .as_ref()
                .is_some_and(|d| d.path == path || d.saving)
            {
                self.activate_tab(&path);
            }
            self.activate_files();
            return;
        }
        let mut doc = crate::editor::draft::OpenDoc::new(path.clone(), &file);
        doc.code.set_indent_choice(self.indent_choice);
        doc.code.vim.set_enabled(self.vim_enabled);
        doc.reveal_line = line;
        // Fresh document: arm the highlight debounce so the initial text is
        // coloured once the quiet period passes.
        doc.highlight_dirty_at = Some(std::time::Instant::now());
        if doc.kind == crate::editor::draft::DocKind::Binary
            && let (Some(host), Some(checkout)) = (&self.media_host, &self.checkout)
        {
            doc.media =
                crate::editor::media::Media::open(&doc.path, &checkout.join(&doc.path), host);
        }
        if self.doc.as_ref().is_some_and(|d| d.saving) {
            // No tab is parked while its save is in flight: the write owns
            // the active slot until the reply lands, so the new document
            // joins as an inactive tab and activates like any parked doc.
            self.doc_reload = None;
            self.tabs.parked.insert(path.clone(), doc);
            if !self.tabs.order.contains(&path) {
                self.tabs.order.push(path.clone());
            }
            self.tabs.recent.retain(|p| p != &path);
            self.tabs.recent.insert(1.min(self.tabs.recent.len()), path);
            self.activate_files();
            return;
        }
        // The draft guard already resolved before this reply was applied,
        // and opening a file parks rather than drops the previous document,
        // so nothing unsaved leaves. A pending reload targeted the OLD doc —
        // never let it land on the new one.
        self.doc_reload = None;
        self.tabs.open(&mut self.doc, doc);
        self.activate_files();
        // The blame annotations describe the file on disk — ask for them
        // now that the document is open.
        crate::editor::blame::sync_visibility(self);
        crate::editor::blame::request(self);
        crate::editor::line_changes::sync_visibility(self);
        crate::editor::line_changes::request(self);
    }

    /// Make the parked tab `path` the active document; `true` when the tab
    /// switched. The active-only pipelines re-arm around the swap —
    /// parked tabs receive no highlight, blame, line-change or disk-check
    /// results, so activation re-requests them.
    pub fn activate_tab(&mut self, path: &str) -> bool {
        if !self.tabs.activate(&mut self.doc, path) {
            return false;
        }
        self.after_active_change();
        true
    }

    /// Activate the tab `delta` positions away in open order, wrapping —
    /// the next/previous-tab chords. `false` with fewer than two tabs.
    pub fn step_tab(&mut self, delta: i32) -> bool {
        if self.tabs.step(&mut self.doc, delta).is_none() {
            return false;
        }
        self.after_active_change();
        true
    }

    /// Close the tab `path`; the caller guards dirty tabs first. The active
    /// tab's right neighbour activates, else the left one, else no document
    /// stays open. Closing a parked tab changes nothing for the active
    /// document, so it re-requests nothing.
    pub fn close_tab(&mut self, path: &str) {
        let was_active = self.doc.as_ref().is_some_and(|d| d.path == path);
        self.tabs.close(&mut self.doc, path);
        if was_active && self.doc.as_ref().is_some_and(|d| d.path == path) {
            // The active doc was never registered as a tab (a direct
            // install): drop it and fall back to the first surviving tab.
            self.doc = None;
            if let Some(next) = self.tabs.order.first().cloned() {
                self.tabs.activate(&mut self.doc, &next);
            }
        }
        if was_active {
            self.after_active_change();
        }
    }

    /// The active-document slot changed (tab switch, close-neighbour,
    /// guard activation): re-arm the highlight debounce, re-request blame
    /// and line changes — they follow the active doc — and probe the file
    /// on disk the way the autorefresh lane's `Req::CheckFile` does.
    fn after_active_change(&mut self) {
        if let Some(doc) = self.doc.as_mut() {
            doc.highlight_dirty_at = Some(std::time::Instant::now());
            doc.highlight_inflight = false;
            doc.code.vim.set_enabled(self.vim_enabled);
        }
        crate::editor::blame::sync_visibility(self);
        crate::editor::blame::request(self);
        crate::editor::line_changes::sync_visibility(self);
        crate::editor::line_changes::request(self);
        let checkable = self.doc.as_ref().is_some_and(|doc| {
            doc.kind == crate::editor::draft::DocKind::Text && !doc.truncated && !doc.saving
        });
        if checkable
            && self.doc_reload.is_none()
            && !self.autorefresh.check_pending
            && let (Some(checkout), Some(key), Some(doc)) = (
                self.checkout.clone(),
                self.checkout_key.clone(),
                self.doc.as_ref(),
            )
        {
            let req = Req::CheckFile {
                path: doc.path.clone(),
                revision: doc.code.revision,
            };
            self.autorefresh.check_pending = self.enqueue_read(checkout, key, req);
        }
        self.sync_quit_dirty();
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
        doc.code.set_indent_choice(self.indent_choice);
        doc.code.vim.set_enabled(self.vim_enabled);
        doc.reveal_line = self.doc.as_ref().and_then(|d| d.reveal_line);
        doc.highlight_dirty_at = Some(std::time::Instant::now());
        self.doc = Some(doc);
        self.sync_quit_dirty();
        // The reloaded file changed on disk: its blame must be re-read.
        crate::editor::blame::sync_visibility(self);
        crate::editor::blame::request(self);
        crate::editor::line_changes::sync_visibility(self);
        crate::editor::line_changes::request(self);
    }

    /// `Req::CheckFile` reply — the autorefresh probe. Applies only while
    /// the doc it probed is still the open one (path), untouched since the
    /// request (revision — an edit during the flight bumps it and drops the
    /// reply), and no save or reload owns the next state. The probe never
    /// writes the buffer for a dirty doc or for a file that stopped being
    /// complete text: those only move `disk_notice`.
    fn apply_check(&mut self, path: &str, revision: u64, file: GitViewFile) {
        if self.doc_reload.is_some() {
            return;
        }
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        if doc.path != path || doc.code.revision != revision || doc.saving {
            return;
        }
        enum Check {
            Gone,
            NotText,
            Same,
            Dirty,
            Replace,
        }
        let check = match (file.kind, file.truncated, file.text.as_deref()) {
            ("missing", ..) => Check::Gone,
            ("text", false, Some(text)) if text == doc.original => Check::Same,
            ("text", false, Some(_)) if doc.dirty() => Check::Dirty,
            ("text", false, Some(_)) => Check::Replace,
            _ => Check::NotText,
        };
        match check {
            Check::Gone => doc.disk_notice = Some(crate::editor::draft::DiskNotice::Deleted),
            Check::NotText => doc.disk_notice = Some(crate::editor::draft::DiskNotice::NotText),
            Check::Dirty => doc.disk_notice = Some(crate::editor::draft::DiskNotice::Changed),
            Check::Same => doc.disk_notice = None,
            Check::Replace => {
                // Clean doc, disk moved: take the disk text silently —
                // baseline AND buffer — keeping the caret where the new
                // length allows. Built like `apply_reload`: `CodeState`
                // has no swap-text-and-keep-the-caret method.
                let (anchor, focus) = (doc.code.edit.anchor, doc.code.edit.focus);
                let (focused, reveal_line) = (doc.code.edit.focused, doc.reveal_line);
                let mut new_doc = crate::editor::draft::OpenDoc::new(path.to_string(), &file);
                new_doc.code.set_indent_choice(self.indent_choice);
                new_doc.code.vim.set_enabled(self.vim_enabled);
                new_doc.reveal_line = reveal_line;
                new_doc.highlight_dirty_at = Some(std::time::Instant::now());
                // Above the revision this check ran at, so a stale probe
                // still in flight drops instead of re-entering this arm.
                new_doc.code.revision = revision + 1;
                let len = new_doc.code.edit.text.len();
                new_doc
                    .code
                    .edit
                    .set_selection(anchor.min(len), focus.min(len));
                new_doc.code.edit.focused = focused;
                self.doc = Some(new_doc);
                self.sync_quit_dirty();
                // The file changed on disk: its blame and bars describe
                // the new text.
                crate::editor::blame::sync_visibility(self);
                crate::editor::blame::request(self);
                crate::editor::line_changes::sync_visibility(self);
                crate::editor::line_changes::request(self);
            }
        }
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
        if self.folder_project
            && matches!(
                self.section,
                Section::Changes | Section::History | Section::Stashes
            )
        {
            return;
        }
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

    /// Apply a new indent-unit override: mirror it into every mounted code
    /// surface and persist through the same project-level settings lane as
    /// the history layout. A refused queue simply retries on the next press.
    pub fn set_indent_choice(&mut self, choice: crate::editor::indent::UnitChoice) {
        self.indent_choice = choice;
        if let Some(doc) = self.doc.as_mut() {
            doc.code.set_indent_choice(choice);
        }
        self.scratch_code.set_indent_choice(choice);
        let _ = enqueue(
            &mut self.backend,
            Job::project(Req::SetSetting {
                key: crate::editor::indent::INDENT_KEY.to_string(),
                value: choice.store_value(),
            }),
        );
    }

    /// Mirror the inline-blame toggle into every mounted code surface.
    pub fn apply_blame_enabled(&mut self, enabled: bool) {
        self.blame_enabled = enabled;
        if let Some(doc) = self.doc.as_mut() {
            doc.code.blame_enabled = enabled;
        }
        self.scratch_code.blame_enabled = enabled;
    }

    /// Apply a new inline-blame toggle: mirror it into every mounted code
    /// surface and persist it through the same project-level settings lane
    /// as the indent override.
    pub fn set_blame_enabled(&mut self, enabled: bool) {
        self.apply_blame_enabled(enabled);
        let _ = enqueue(
            &mut self.backend,
            Job::project(Req::SetSetting {
                key: crate::editor::blame::BLAME_KEY.to_string(),
                value: if enabled { "1".into() } else { "0".into() },
            }),
        );
    }

    /// Mirror the Vim switch into every mounted code surface. Turning it off
    /// returns each document to Normal and drops half-typed keys.
    pub fn apply_vim_enabled(&mut self, enabled: bool) {
        self.vim_enabled = enabled;
        if let Some(doc) = self.doc.as_mut() {
            doc.code.vim.set_enabled(enabled);
            if !enabled {
                doc.code.vim_search = None;
                doc.code.vim_cmd = None;
            }
        }
        self.scratch_code.vim.set_enabled(enabled);
        if !enabled {
            self.scratch_code.vim_search = None;
            self.scratch_code.vim_cmd = None;
        }
    }

    /// Apply a new Vim switch and persist it through the same settings lane
    /// as the indent override.
    pub fn set_vim_enabled(&mut self, enabled: bool) {
        self.apply_vim_enabled(enabled);
        let _ = enqueue(
            &mut self.backend,
            Job::project(Req::SetSetting {
                key: crate::editor::vim::VIM_KEY.to_string(),
                value: if enabled { "1".into() } else { "0".into() },
            }),
        );
    }

    /// Queue a worktree-list read for `project_id`, marking the sidebar's
    /// per-project entry loading when the job is accepted. The reply lands
    /// in `sidebar_worktrees` — and in `worktrees` too when `project_id` is
    /// the selected project. `true` means a reply is coming (fresh send or
    /// already in flight); `false` means the bounded queue refused it.
    pub fn request_worktrees(&mut self, project_id: &str) -> bool {
        if self
            .sidebar_worktrees
            .get(project_id)
            .is_some_and(|e| e.loading)
        {
            return true;
        }
        if self
            .backend
            .send(Job::project(Req::Worktrees(project_id.to_string())))
        {
            let entry = self
                .sidebar_worktrees
                .entry(project_id.to_string())
                .or_default();
            entry.loading = true;
            entry.error = None;
            true
        } else {
            false
        }
    }

    /// Initial project list + worktree list for the selected project.
    /// A refused `Projects` job must not leave `projects_loading` stuck —
    /// the sidebar would spin forever with no error to show.
    pub fn refresh_projects(&mut self) {
        if self.backend.send(Job::project(Req::Projects)) {
            self.projects_loading = true;
            self.projects_error = None;
        } else {
            self.projects_loading = false;
            self.projects_error =
                Some("the editor queue is full; projects were not requested".into());
        }
        if let Some(pid) = self.project_id.clone() {
            if self.request_worktrees(&pid) {
                self.worktrees_loading = true;
                self.worktrees_error = None;
            } else {
                self.worktrees_loading = false;
                self.worktrees_error =
                    Some("the editor queue is full; worktrees were not requested".into());
            }
        }
    }
}

/// Request-kind key for generation tracking; writes and project calls map
/// to kinds that never carry a generation.
/// The ignored entries with every opened directory replaced by its entries
/// (an opened directory with nothing in it stays a leaf).
fn expand_ignored(
    ignored: &[String],
    open: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out = Vec::with_capacity(ignored.len());
    let mut stack: Vec<&String> = ignored.iter().rev().collect();
    while let Some(entry) = stack.pop() {
        match entry
            .strip_suffix('/')
            .and_then(|dir| open.get(dir))
            .filter(|children| !children.is_empty())
        {
            Some(children) => stack.extend(children.iter().rev()),
            None => out.push(entry.clone()),
        }
    }
    out
}

/// Every folder row of `model`.
fn folder_ids(model: &rune_kit::TreeModel<rune_kit::FileNode<FileMark>>) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = model.children(None).map(str::to_string).collect();
    while let Some(id) = stack.pop() {
        if model.has_children(&id) {
            stack.extend(model.children(Some(&id)).map(str::to_string));
            out.push(id);
        }
    }
    out
}

/// The folders of `model` that are open.
fn expanded_ids(model: &rune_kit::TreeModel<rune_kit::FileNode<FileMark>>) -> Vec<String> {
    let mut ids = folder_ids(model);
    ids.retain(|id| model.is_expanded(id));
    ids
}

fn kind_key(req: &Req) -> &'static str {
    match req {
        Req::Changes => "changes",
        // Own lane: a poll must never supersede (or be superseded by) the
        // manual `Changes` read — a dropped reply would strand its spinner.
        Req::ChangesPoll => "changespoll",
        Req::CurrentBranch => "currentbranch",
        Req::Diff {
            commit: Some(_), ..
        } => "commitdiff",
        Req::Diff { .. } => "diff",
        Req::History { .. } => "history",
        Req::Branches => "branches",
        Req::FileList => "filelist",
        Req::Search { .. } => "search",
        Req::ConceptSearch { .. } => "conceptsearch",
        Req::GoToSearch { .. } => "gotosarch",
        Req::ConfigFiles { .. } => "configfiles",
        Req::Highlight { .. } => "highlight",
        Req::File { .. } => "file",
        Req::ReloadFile { .. } => "reloadfile",
        // Own lane: a doc open or reload in flight must never be superseded
        // by the disk-check probe (a dropped File reply strands `doc_load`).
        Req::CheckFile { .. } => "checkfile",
        Req::Stashes => "stashes",
        Req::StashDiff { .. } => "stashdiff",
        Req::Blame { .. } => "blame",
        Req::LineChanges { .. } => "linechanges",
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
mod tests;

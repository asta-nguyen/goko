//! The request/response commands the webview calls. Each is a thin wrapper over a `Bridge`
//! method so tests can call the method directly.

use crate::acp::{self, AcpAgent, AcpHub, AcpOverride, AcpStatus};
use crate::agents::{self, AgentInfo};
use crate::bridge::Bridge;
use crate::daemon_client;
use crate::error::{BridgeError, Code};
use crate::git_view::{
    self, GitViewBranch, GitViewChanges, GitViewDiff, GitViewFile, GitViewHistory, GitViewSearch,
    GitViewStash, GitViewTree,
};
use crate::git_write::{self, GitCommitResult, GitPushResult};
use crate::store::{SessionMeta, Storage, Store};
use crate::worktrees::{
    self, EnvCopyReport, EnvFileCandidate, WorktreeBranches, WorktreeInfo, WorktreeList,
};
use app_core::projects;
pub use app_core::projects::ProjectView;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protocol::message::{ErrorCode, Request};
use protocol::session::{SessionId, SessionInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub connected: bool,
    pub live_cwd_supported: Option<bool>,
    pub default_shell: String,
    pub home: String,
}

/// Everything `session_create` accepts. Exactly one of `command`/`agent` and exactly one
/// of `project_id`/`cwd` must be `Some`.
pub struct CreateArgs {
    pub command: Option<String>,
    pub agent: Option<String>,
    pub project_id: Option<String>,
    pub checkout: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// The validated launch: what to spawn and how to describe it in metadata.
struct Launch {
    argv: Vec<String>,
    cwd: String,
    project_id: Option<String>,
    /// The adapter id; `None` for a free-form command.
    agent: Option<String>,
}

impl Bridge {
    /// Validate `args`, launch through the daemon (the only PTY authority), then record
    /// the session's metadata best-effort. A `command` is split into `argv` like a shell
    /// would, but never run by one.
    pub async fn session_create(
        &self,
        storage: &Storage,
        args: CreateArgs,
    ) -> Result<SessionInfo, BridgeError> {
        let _session_launch = self.worktree_operations().read().await;
        let launch = resolve_launch(storage, &args)?;
        let Launch {
            argv,
            cwd,
            project_id,
            agent,
        } = launch;
        let reply = self
            .call(|id| Request::SessionCreate {
                id,
                argv,
                cwd,
                cols: args.cols,
                rows: args.rows,
                title: args.title.unwrap_or_default(),
            })
            .await?;
        let session: SessionInfo = parse(reply)?;
        if let Ok(store) = storage.get() {
            record(store, &session, &project_id, agent.as_deref());
        }
        Ok(session)
    }

    pub async fn session_stop(
        &self,
        session_id: SessionId,
        force: bool,
    ) -> Result<(), BridgeError> {
        self.call(|id| Request::SessionStop {
            id,
            session_id,
            force,
        })
        .await
        .map(drop)
    }

    pub async fn session_delete(
        &self,
        storage: &Storage,
        session_id: SessionId,
    ) -> Result<(), BridgeError> {
        if let Err(error) = self
            .call(|id| Request::SessionDelete { id, session_id })
            .await
        {
            if error.code != Code::Daemon(ErrorCode::NotFound) {
                if error.code == Code::Daemon(ErrorCode::Malformed)
                    && error.message.contains("unknown variant `session.delete`")
                {
                    return Err(BridgeError::new(
                        error.code,
                        "Daemon is outdated; save live sessions, restart it, then retry closing this session.",
                    ));
                }
                return Err(error);
            }
        }
        storage
            .get()?
            .delete_session_meta(&session_id.as_uuid().to_string())
    }

    pub async fn session_list(&self) -> Result<Vec<SessionInfo>, BridgeError> {
        #[derive(Deserialize)]
        struct Listed {
            sessions: Vec<SessionInfo>,
        }
        let reply = self.call(|id| Request::SessionList { id }).await?;
        parse::<Listed>(reply).map(|listed| listed.sessions)
    }

    pub async fn terminal_write(
        &self,
        session_id: SessionId,
        data: &str,
    ) -> Result<(), BridgeError> {
        let data_b64 = STANDARD.encode(data);
        self.call(|id| Request::TerminalWrite {
            id,
            session_id,
            data_b64,
        })
        .await
        .map(drop)
    }

    pub async fn terminal_resize(
        &self,
        session_id: SessionId,
        cols: u16,
        rows: u16,
    ) -> Result<(), BridgeError> {
        self.call(|id| Request::TerminalResize {
            id,
            session_id,
            cols,
            rows,
        })
        .await
        .map(drop)
    }

    /// Never connects or starts the daemon.
    pub fn daemon_status(&self) -> DaemonStatus {
        let client = self.current();
        DaemonStatus {
            connected: client.is_some(),
            live_cwd_supported: client.map(|client| client.supports_live_cwd()),
            default_shell: default_shell(std::env::var_os("SHELL")),
            home: self.home().display().to_string(),
        }
    }
}

/// Everything rejected here is rejected before the daemon is reached.
fn resolve_launch(storage: &Storage, args: &CreateArgs) -> Result<Launch, BridgeError> {
    if args.command.is_some() == args.agent.is_some() {
        return Err(invalid("pass exactly one of `command` or `agent`"));
    }
    if args.project_id.is_some() == args.cwd.is_some() {
        return Err(invalid("pass exactly one of `project_id` or `cwd`"));
    }
    if args.checkout.is_some() && args.project_id.is_none() {
        return Err(invalid("`checkout` is only valid with `project_id`"));
    }
    let (argv, agent) = match &args.agent {
        Some(agent) => {
            let (goko_home, original_statusline) = if agent == "claude" {
                let home = daemon_client::home_dir()?;
                let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                    .filter(|dir| !dir.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"))
                    });
                let original = config_dir
                    .and_then(|dir| agents::original_statusline(&dir.join("settings.json")));
                (home, original)
            } else {
                (PathBuf::new(), None)
            };
            (
                agents::argv_for(
                    agent,
                    std::env::var_os("PATH").as_deref(),
                    &default_shell(std::env::var_os("SHELL")),
                    &goko_home,
                    original_statusline.as_deref(),
                )?,
                Some(agent.clone()),
            )
        }
        None => {
            let command = args.command.as_ref().expect("exactly one checked above");
            (
                shlex::split(command)
                    .ok_or_else(|| invalid("the command has an unbalanced quote"))?,
                None,
            )
        }
    };
    let cwd = match &args.project_id {
        Some(project_id) => {
            let root = project_root(storage, project_id)?;
            match &args.checkout {
                Some(checkout) => projects::project_checkout(&root, checkout)?,
                None => root,
            }
        }
        None => args.cwd.clone().expect("exactly one checked above"),
    };
    Ok(Launch {
        argv,
        cwd,
        project_id: args.project_id.clone(),
        agent,
    })
}

/// The registered project's root, which must still exist on disk.
fn project_root(storage: &Storage, project_id: &str) -> Result<String, BridgeError> {
    projects::project_root(storage.get()?, project_id)
}

/// Best-effort: an insert or settings failure is logged, never propagated — the process
/// keeps running and its session appears unassigned.
fn record(store: &Store, session: &SessionInfo, project_id: &Option<String>, agent: Option<&str>) {
    if let Err(error) = store.insert_session_meta(&SessionMeta {
        session_id: session.id.as_uuid().to_string(),
        project_id: project_id.clone(),
        agent: agent.unwrap_or("custom").into(),
        argv: session.argv.clone(),
        cwd: session.cwd.clone(),
        title: session.title.clone(),
        created_at: session.started_at,
    }) {
        eprintln!("goko: the session's metadata was not recorded: {error}");
    }
    if let Some(agent) = agent {
        if let Err(error) = store.set_setting("last_agent", agent) {
            eprintln!("goko: last_agent was not saved: {error}");
        }
    }
}

pub(crate) fn default_shell(shell: Option<OsString>) -> String {
    shell.filter(|shell| !shell.is_empty()).map_or_else(
        || "/bin/zsh".to_owned(),
        |shell| shell.to_string_lossy().into_owned(),
    )
}

fn parse<T: DeserializeOwned>(reply: Value) -> Result<T, BridgeError> {
    serde_json::from_value(reply).map_err(|error| {
        BridgeError::new(
            Code::Daemon(ErrorCode::Malformed),
            format!("unexpected reply from gokod: {error}"),
        )
    })
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn session_create(
    bridge: State<'_, Bridge>,
    storage: State<'_, Storage>,
    command: Option<String>,
    agent: Option<String>,
    project_id: Option<String>,
    checkout: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<SessionInfo, BridgeError> {
    bridge
        .session_create(
            &storage,
            CreateArgs {
                command,
                agent,
                project_id,
                checkout,
                cwd,
                title,
                cols,
                rows,
            },
        )
        .await
}

#[tauri::command]
pub fn worktree_list(
    storage: State<'_, Storage>,
    project_id: String,
) -> Result<WorktreeList, BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::list(Path::new(&root))
}

#[tauri::command]
pub fn worktree_branches(
    storage: State<'_, Storage>,
    project_id: String,
) -> Result<WorktreeBranches, BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::branches(Path::new(&root))
}

#[tauri::command]
pub fn worktree_create(
    storage: State<'_, Storage>,
    project_id: String,
    path: String,
    mode: String,
    branch: String,
    base: Option<String>,
) -> Result<WorktreeInfo, BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::create(Path::new(&root), &path, &mode, &branch, base.as_deref())
}

#[tauri::command]
pub async fn worktree_remove(
    bridge: State<'_, Bridge>,
    storage: State<'_, Storage>,
    project_id: String,
    path: String,
) -> Result<(), BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::remove(&bridge, Path::new(&root), &path).await
}

#[tauri::command]
pub fn env_file_candidates(
    storage: State<'_, Storage>,
    project_id: String,
    source_path: String,
) -> Result<Vec<EnvFileCandidate>, BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::env_candidates(Path::new(&root), &source_path)
}

#[tauri::command]
pub fn env_copy(
    storage: State<'_, Storage>,
    project_id: String,
    source_path: String,
    target_path: String,
) -> Result<EnvCopyReport, BridgeError> {
    let root = project_root(&storage, &project_id)?;
    worktrees::env_copy(Path::new(&root), &source_path, &target_path)
}

/// Checkout for a Git view command: registered project, root-or-listed
/// worktree, and a real non-bare Git worktree root.
fn git_view_checkout(
    storage: &Storage,
    project_id: &str,
    checkout_path: &str,
) -> Result<PathBuf, BridgeError> {
    projects::git_checkout(storage.get()?, project_id, checkout_path)
}

#[tauri::command]
pub fn git_view_changes(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
) -> Result<GitViewChanges, BridgeError> {
    git_view::changes(&git_view_checkout(&storage, &project_id, &checkout_path)?)
}

#[tauri::command]
pub fn git_view_diff(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    layer: String,
    path: Option<String>,
    commit: Option<String>,
) -> Result<GitViewDiff, BridgeError> {
    git_view::diff(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &layer,
        path.as_deref(),
        commit.as_deref(),
    )
}

#[tauri::command]
pub fn git_view_history(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    skip: Option<u64>,
    branch: Option<String>,
) -> Result<GitViewHistory, BridgeError> {
    git_view::history(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        skip.unwrap_or(0),
        branch.as_deref(),
    )
}

#[tauri::command]
pub fn git_view_branches(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
) -> Result<Vec<GitViewBranch>, BridgeError> {
    git_view::branches(&git_view_checkout(&storage, &project_id, &checkout_path)?)
}

#[tauri::command]
pub fn git_view_file_list(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
) -> Result<Vec<String>, BridgeError> {
    git_view::file_list(&git_view_checkout(&storage, &project_id, &checkout_path)?)
}

// `async` keeps `git grep` off the main thread; a sync command would freeze the window on a big repo.
#[tauri::command(async)]
pub fn git_view_search(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    query: String,
) -> Result<GitViewSearch, BridgeError> {
    git_view::search(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &query,
    )
}

#[tauri::command]
pub fn git_view_tree(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    directory: String,
) -> Result<GitViewTree, BridgeError> {
    git_view::tree(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &directory,
    )
}

#[tauri::command]
pub fn git_view_file(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    path: String,
) -> Result<GitViewFile, BridgeError> {
    git_view::file(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &path,
    )
}

#[tauri::command]
pub fn git_view_file_save(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    path: String,
    expected_text: String,
    text: String,
) -> Result<GitViewFile, BridgeError> {
    git_view::save_file(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &path,
        &expected_text,
        &text,
    )
}

#[tauri::command]
pub fn git_view_stashes(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
) -> Result<Vec<GitViewStash>, BridgeError> {
    git_view::stashes(&git_view_checkout(&storage, &project_id, &checkout_path)?)
}

#[tauri::command]
pub fn git_view_stash_diff(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    stash_id: String,
) -> Result<GitViewDiff, BridgeError> {
    git_view::stash_diff(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &stash_id,
    )
}

#[tauri::command]
pub fn git_write_stage(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    paths: Option<Vec<String>>,
) -> Result<(), BridgeError> {
    git_write::stage(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        paths.as_deref(),
    )
}

#[tauri::command]
pub fn git_write_unstage(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    paths: Option<Vec<String>>,
) -> Result<(), BridgeError> {
    git_write::unstage(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        paths.as_deref(),
    )
}

#[tauri::command]
pub fn git_write_discard(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    layer: String,
    path: String,
) -> Result<(), BridgeError> {
    git_write::discard(
        &git_view_checkout(&storage, &project_id, &checkout_path)?,
        &layer,
        &path,
    )
}

// `async` on a blocking thread keeps hooks and the network off the async runtime.
#[tauri::command]
pub async fn git_write_commit(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    message: String,
) -> Result<GitCommitResult, BridgeError> {
    let checkout = git_view_checkout(&storage, &project_id, &checkout_path)?;
    tokio::task::spawn_blocking(move || git_write::commit(&checkout, &message))
        .await
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("git commit could not run: {error}"),
            )
        })?
}

// `async` on a blocking thread keeps the push (and its 120 s deadline) off the async runtime.
#[tauri::command]
pub async fn git_write_push(
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
) -> Result<GitPushResult, BridgeError> {
    let checkout = git_view_checkout(&storage, &project_id, &checkout_path)?;
    tokio::task::spawn_blocking(move || git_write::push(&checkout))
        .await
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("git push could not run: {error}"),
            )
        })?
}

#[tauri::command]
pub async fn session_stop(
    bridge: State<'_, Bridge>,
    id: SessionId,
    force: bool,
) -> Result<(), BridgeError> {
    bridge.session_stop(id, force).await
}

#[tauri::command]
pub async fn session_delete(
    bridge: State<'_, Bridge>,
    storage: State<'_, Storage>,
    id: SessionId,
) -> Result<(), BridgeError> {
    bridge.session_delete(&storage, id).await
}

#[tauri::command]
pub async fn session_list(bridge: State<'_, Bridge>) -> Result<Vec<SessionInfo>, BridgeError> {
    bridge.session_list().await
}

#[tauri::command]
pub async fn terminal_write(
    bridge: State<'_, Bridge>,
    activity: State<'_, crate::activity::ActivityState>,
    id: SessionId,
    data: String,
) -> Result<(), BridgeError> {
    activity.note_input(&id.as_uuid().to_string(), &data);
    bridge.terminal_write(id, &data).await
}

#[tauri::command]
pub async fn terminal_resize(
    bridge: State<'_, Bridge>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<(), BridgeError> {
    bridge.terminal_resize(id, cols, rows).await
}

#[tauri::command]
pub fn daemon_status(bridge: State<'_, Bridge>) -> DaemonStatus {
    bridge.daemon_status()
}

#[tauri::command]
pub async fn project_add(
    storage: State<'_, Storage>,
    path: String,
) -> Result<ProjectView, BridgeError> {
    projects::add_project(storage.get()?, &path)
}

#[tauri::command]
pub async fn project_list(storage: State<'_, Storage>) -> Result<Vec<ProjectView>, BridgeError> {
    projects::list_projects(storage.get()?)
}

#[tauri::command]
pub async fn project_remove(storage: State<'_, Storage>, id: String) -> Result<(), BridgeError> {
    projects::remove_project(storage.get()?, &id)
}

#[tauri::command]
pub fn agent_list() -> Vec<AgentInfo> {
    agents::discover(
        std::env::var_os("PATH").as_deref(),
        &default_shell(std::env::var_os("SHELL")),
    )
}

#[tauri::command]
pub async fn session_meta_list(
    storage: State<'_, Storage>,
) -> Result<Vec<SessionMeta>, BridgeError> {
    storage.get()?.list_session_meta()
}

#[tauri::command]
pub async fn settings_get(
    storage: State<'_, Storage>,
    key: String,
) -> Result<Option<String>, BridgeError> {
    storage.get()?.get_setting(&key)
}

#[tauri::command]
pub async fn settings_set(
    storage: State<'_, Storage>,
    key: String,
    value: String,
) -> Result<(), BridgeError> {
    storage.get()?.set_setting(&key, &value)
}

#[tauri::command]
pub async fn acp_agents_get(storage: State<'_, Storage>) -> Result<Vec<AcpAgent>, BridgeError> {
    acp::agents_get(&storage)
}

#[tauri::command]
pub async fn acp_agents_set(
    hub: State<'_, AcpHub>,
    storage: State<'_, Storage>,
    overrides: Vec<AcpOverride>,
) -> Result<Vec<AcpAgent>, BridgeError> {
    acp::agents_set(&storage, &hub, overrides).await
}

#[tauri::command]
pub async fn acp_connect(
    hub: State<'_, AcpHub>,
    storage: State<'_, Storage>,
    agent_id: String,
) -> Result<(), BridgeError> {
    let config = acp::agent_config(&storage, &agent_id)?;
    hub.connect(&config).await
}

#[tauri::command]
pub async fn acp_disconnect(hub: State<'_, AcpHub>, agent_id: String) -> Result<(), BridgeError> {
    hub.disconnect(&agent_id).await
}

#[tauri::command]
pub fn acp_status(hub: State<'_, AcpHub>) -> BTreeMap<String, AcpStatus> {
    hub.status()
}

#[tauri::command]
pub async fn acp_generate_commit_message(
    hub: State<'_, AcpHub>,
    storage: State<'_, Storage>,
    project_id: String,
    checkout_path: String,
    agent_id: String,
) -> Result<String, BridgeError> {
    let config = acp::agent_config(&storage, &agent_id)?;
    let checkout = git_view_checkout(&storage, &project_id, &checkout_path)?;
    hub.generate_commit_message(config, project_id, checkout_path, checkout)
        .await
}

#[tauri::command]
pub fn acp_cancel(
    hub: State<'_, AcpHub>,
    project_id: String,
    checkout_path: String,
) -> Result<(), BridgeError> {
    hub.cancel(&project_id, &checkout_path);
    Ok(())
}

#[cfg(test)]
mod tests;

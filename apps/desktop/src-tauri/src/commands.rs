//! The request/response commands the webview calls. Each is a thin wrapper over a `Bridge`
//! method so tests can call the method directly.

use crate::agents::{self, AgentInfo};
use crate::bridge::Bridge;
use crate::error::{BridgeError, Code};
use crate::store::{Project, SessionMeta, Storage, Store};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protocol::message::{ErrorCode, Request};
use protocol::session::{SessionId, SessionInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub connected: bool,
    pub default_shell: String,
    pub home: String,
}

/// A project as the webview sees it: the stored row plus `missing`, computed at read time.
#[derive(Debug, Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    project: Project,
    missing: bool,
}

impl ProjectView {
    fn of(project: Project) -> Self {
        Self {
            missing: !Path::new(&project.root).is_dir(),
            project,
        }
    }
}

/// Everything `session_create` accepts. Exactly one of `command`/`agent` and exactly one
/// of `project_id`/`cwd` must be `Some`.
pub struct CreateArgs {
    pub command: Option<String>,
    pub agent: Option<String>,
    pub project_id: Option<String>,
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
        DaemonStatus {
            connected: self.current().is_some(),
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
    let (argv, agent) = match &args.agent {
        Some(agent) => (
            agents::argv_for(
                agent,
                std::env::var_os("PATH").as_deref(),
                &default_shell(std::env::var_os("SHELL")),
            )?,
            Some(agent.clone()),
        ),
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
        Some(project_id) => project_root(storage, project_id)?,
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
    let project = storage
        .get()?
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| not_found(format!("no project '{project_id}' is registered")))?;
    if !Path::new(&project.root).is_dir() {
        return Err(invalid(format!(
            "the project root {} no longer exists",
            project.root
        )));
    }
    Ok(project.root)
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

fn not_found(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::NotFound), message)
}

/// Canonicalize `path` and register it; a file or a missing path is `invalid_argument`.
fn add_project(store: &Store, path: &str) -> Result<ProjectView, BridgeError> {
    let root = std::fs::canonicalize(path)
        .map_err(|error| invalid(format!("cannot use '{path}': {error}")))?;
    if !root.is_dir() {
        return Err(invalid(format!("'{path}' is not a directory")));
    }
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    store
        .add_project(&name, &root.display().to_string())
        .map(ProjectView::of)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn session_create(
    bridge: State<'_, Bridge>,
    storage: State<'_, Storage>,
    command: Option<String>,
    agent: Option<String>,
    project_id: Option<String>,
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
                cwd,
                title,
                cols,
                rows,
            },
        )
        .await
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
pub async fn session_list(bridge: State<'_, Bridge>) -> Result<Vec<SessionInfo>, BridgeError> {
    bridge.session_list().await
}

#[tauri::command]
pub async fn terminal_write(
    bridge: State<'_, Bridge>,
    id: SessionId,
    data: String,
) -> Result<(), BridgeError> {
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
    add_project(storage.get()?, &path)
}

#[tauri::command]
pub async fn project_list(storage: State<'_, Storage>) -> Result<Vec<ProjectView>, BridgeError> {
    Ok(storage
        .get()?
        .list_projects()?
        .into_iter()
        .map(ProjectView::of)
        .collect())
}

#[tauri::command]
pub async fn project_remove(storage: State<'_, Storage>, id: String) -> Result<(), BridgeError> {
    match storage.get()?.remove_project(&id)? {
        true => Ok(()),
        false => Err(not_found(format!("no project '{id}' is registered"))),
    }
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

#[cfg(test)]
mod tests;

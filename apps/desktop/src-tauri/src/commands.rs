//! The request/response commands the webview calls. Each is a thin wrapper over a `Bridge`
//! method so tests can call the method directly.

use crate::bridge::Bridge;
use crate::error::{BridgeError, Code};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protocol::message::{ErrorCode, Request};
use protocol::session::{SessionId, SessionInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub connected: bool,
    pub default_shell: String,
    pub home: String,
}

impl Bridge {
    /// `command` is split into `argv` like a shell would, but never run by one.
    pub async fn session_create(
        &self,
        command: &str,
        cwd: String,
        cols: u16,
        rows: u16,
        title: Option<String>,
    ) -> Result<SessionInfo, BridgeError> {
        let argv = shlex::split(command).ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::InvalidArgument),
                "the command has an unbalanced quote",
            )
        })?;
        let reply = self
            .call(|id| Request::SessionCreate {
                id,
                argv,
                cwd,
                cols,
                rows,
                title: title.unwrap_or_default(),
            })
            .await?;
        parse(reply)
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

fn default_shell(shell: Option<OsString>) -> String {
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

#[tauri::command]
pub async fn session_create(
    bridge: State<'_, Bridge>,
    command: String,
    cwd: String,
    cols: u16,
    rows: u16,
    title: Option<String>,
) -> Result<SessionInfo, BridgeError> {
    bridge
        .session_create(&command, cwd, cols, rows, title)
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

#[cfg(test)]
mod tests;

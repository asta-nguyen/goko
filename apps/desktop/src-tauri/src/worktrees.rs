//! Git worktree discovery and lifecycle for registered projects.
//!
//! The shared Git runner, read discovery, create, and env-copy operations live
//! in `app_core::worktrees` and are re-exported here. `remove` stays
//! desktop-owned: it must ask the daemon about live sessions before deleting
//! a checkout, which a Tauri-free core cannot do.

pub use app_core::worktrees::*;

use crate::bridge::Bridge;
use crate::error::{BridgeError, Code};
use protocol::message::{ErrorCode, Request};
use protocol::session::{SessionInfo, SessionState};
use serde::Deserialize;
use std::fs;
use std::path::Path;

pub async fn remove(bridge: &Bridge, root: &Path, path: &str) -> Result<(), BridgeError> {
    let _remove_checkout = bridge.worktree_operations().write().await;
    let _process_remove_checkout = bridge.worktree_operation_guard(true).await?;
    let listed = list(root)?;
    if !listed.repo {
        return Err(invalid("the project root is not a Git worktree"));
    }
    let wanted = normalize_path(&lexical_absolute(root, Path::new(path)))?;
    let target = listed
        .worktrees
        .into_iter()
        .find(|worktree| {
            normalize_path(Path::new(&worktree.path)).ok().as_deref() == Some(wanted.as_path())
        })
        .ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::NotFound),
                format!("{path} is not a linked worktree of this project"),
            )
        })?;
    if target.is_root {
        return Err(invalid("the project root checkout cannot be removed"));
    }
    if target.missing {
        return Err(invalid(
            "the worktree directory is missing; restore it or prune its Git entry manually",
        ));
    }
    let changed = target
        .changed
        .ok_or_else(|| invalid("cannot confirm that the worktree is clean"))?;
    if changed > 0 {
        return Err(invalid(format!(
            "cannot remove a dirty worktree ({changed} changed files)"
        )));
    }
    let target_path = fs::canonicalize(&target.path).map_err(|error| invalid(error.to_string()))?;
    let client = bridge.client().await?;
    if !client.supports_live_cwd() {
        return Err(invalid(format!(
            "the connected gokod {} cannot report live session directories; update and restart it manually before removing worktrees",
            client.daemon_version()
        )));
    }
    #[derive(Deserialize)]
    struct Listed {
        sessions: Vec<SessionInfo>,
    }
    let reply = client.request(|id| Request::SessionList { id }).await?;
    let sessions = serde_json::from_value::<Listed>(reply)
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Malformed),
                format!("unexpected reply from gokod: {error}"),
            )
        })?
        .sessions;
    if sessions
        .iter()
        .any(|session| !matches!(session.state, SessionState::Exited | SessionState::Failed))
    {
        return Err(invalid(
            "close all running Goko sessions before removing any worktree",
        ));
    }
    let mut attached = 0;
    for session in &sessions {
        let cwd = Path::new(&session.cwd);
        if !cwd.is_absolute() {
            return Err(invalid(
                "cannot confirm the current directory of every session; close or delete sessions with an unknown directory first",
            ));
        }
        let cwd = fs::canonicalize(cwd).map_err(|_| {
            invalid(
                "cannot confirm the current directory of every session; close or delete sessions with an unknown directory first",
            )
        })?;
        if !cwd.is_dir() {
            return Err(invalid(
                "cannot confirm the current directory of every session; close or delete sessions with an unknown directory first",
            ));
        }
        if cwd.starts_with(&target_path) {
            attached += 1;
        }
    }
    if attached > 0 {
        return Err(invalid(format!(
            "cannot remove a worktree with {attached} attached session(s)"
        )));
    }
    let target_arg = target_path
        .to_str()
        .ok_or_else(|| invalid("the worktree path must be valid UTF-8"))?;
    git(root, &["worktree", "remove", target_arg])?;
    Ok(())
}

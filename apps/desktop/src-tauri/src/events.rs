//! Keeps the daemon connection up and forwards its session events to the webview.

use crate::bridge::Bridge;
use protocol::message::{Event, Request};
use protocol::session::SessionState;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::sleep;

/// Delivers a named event with a JSON payload to the webview.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Pause before looking for the daemon again after a connection ended or failed.
const RETRY_PAUSE: Duration = Duration::from_millis(500);

pub fn spawn_pump(bridge: Bridge, emit: Emit) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(run_pump(bridge, emit))
}

/// Connect (starting the daemon if needed), subscribe, and forward events; when the
/// connection drops say so and start over. Runs until the task is aborted.
pub async fn run_pump(bridge: Bridge, emit: Emit) {
    loop {
        if serve(&bridge, &emit).await {
            status(&emit, false);
        }
        sleep(RETRY_PAUSE).await;
    }
}

/// Follow one connection to its end. True when it was announced as connected, so the
/// caller must announce the loss.
async fn serve(bridge: &Bridge, emit: &Emit) -> bool {
    let Ok(client) = bridge.client().await else {
        return false;
    };
    // Listen before subscribing so no event slips between the two.
    let Ok(mut events) = client.events() else {
        return false;
    };
    if client
        .request(|id| Request::EventsSubscribe { id })
        .await
        .is_err()
    {
        return false;
    }
    status(emit, true);
    loop {
        match events.recv().await {
            Ok(Event::SessionUpdated { session }) => {
                let exited = session.state == SessionState::Exited;
                let session = json!(session);
                emit("session://updated", session.clone());
                // The webview wants the whole session here, not the daemon's
                // `session.exited{session_id, exit_code}`, which is therefore ignored.
                if exited {
                    emit("session://exited", session);
                }
            }
            Ok(_) => {}
            // Events were dropped: `connected:true` makes the webview refetch.
            Err(RecvError::Lagged(_)) => status(emit, true),
            Err(RecvError::Closed) => return true,
        }
    }
}

fn status(emit: &Emit, connected: bool) {
    emit("daemon://status", json!({ "connected": connected }));
}

#[cfg(test)]
mod tests;

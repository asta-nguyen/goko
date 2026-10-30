//! Terminal output streaming.
//!
//! One task per subscribed session forwards the daemon's output to the webview. A single
//! actor starts and stops those tasks in the order the requests were queued, so a late
//! `terminal_unsubscribe` can never cancel a newer `terminal_subscribe`.
//!
//! Output reaches the webview as `[u64 BE seq][bytes]`. The task remembers how far it
//! delivered; whenever it must subscribe again (the daemon sent `terminal.lagged`, this
//! consumer fell behind, or the connection was replaced) the replayed prefix it already
//! delivered is trimmed away, so the webview never sees a byte twice.

use crate::bridge::Bridge;
use crate::daemon_client::{DaemonClient, Output};
use crate::error::{BridgeError, Code};
use protocol::message::{Event, Request};
use protocol::session::SessionId;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// Receives one framed message; false means the webview closed its channel.
pub type Sink = Box<dyn Fn(Vec<u8>) -> bool + Send + Sync>;

const RETRY_PAUSE: Duration = Duration::from_millis(500);
const UNSUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) enum Command {
    Subscribe {
        session: SessionId,
        sink: Sink,
        started: oneshot::Sender<Result<(), BridgeError>>,
    },
    Unsubscribe {
        session: SessionId,
    },
}

impl Bridge {
    /// Stream `session` into `sink`, replacing any earlier stream of that session. Returns
    /// once the daemon accepted the subscription.
    pub async fn terminal_subscribe(
        &self,
        session: SessionId,
        sink: Sink,
    ) -> Result<(), BridgeError> {
        let (started, answer) = oneshot::channel();
        let queue = self.terminals().get_or_init(|| {
            let (queue, commands) = mpsc::unbounded_channel();
            tokio::spawn(actor(self.clone(), commands));
            queue
        });
        let command = Command::Subscribe {
            session,
            sink,
            started,
        };
        if queue.send(command).is_err() {
            return Err(BridgeError::new(
                Code::Disconnected,
                "the terminal bridge stopped",
            ));
        }
        // Cancelled by a later unsubscribe or subscribe: nothing is left to report.
        answer.await.unwrap_or(Ok(()))
    }

    /// Stop streaming `session`. Returns before the stop happens; it is queued behind every
    /// earlier subscribe and unsubscribe.
    pub fn terminal_unsubscribe(&self, session: SessionId) {
        if let Some(queue) = self.terminals().get() {
            let _ = queue.send(Command::Unsubscribe { session });
        }
    }
}

async fn actor(bridge: Bridge, mut commands: mpsc::UnboundedReceiver<Command>) {
    let mut streams: HashMap<SessionId, JoinHandle<()>> = HashMap::new();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Subscribe {
                session,
                sink,
                started,
            } => {
                // The new stream's `attach` clears the daemon side of the old one.
                if let Some(old) = streams.remove(&session) {
                    old.abort();
                }
                let task = tokio::spawn(stream(bridge.clone(), session, sink, started));
                streams.insert(session, task);
            }
            Command::Unsubscribe { session } => stop(&bridge, &mut streams, session).await,
        }
    }
}

async fn stop(
    bridge: &Bridge,
    streams: &mut HashMap<SessionId, JoinHandle<()>>,
    session: SessionId,
) {
    if let Some(task) = streams.remove(&session) {
        task.abort();
        unsubscribe_from_daemon(bridge.current(), session).await;
    }
}

/// Never starts the daemon just to tell it to stop streaming.
async fn unsubscribe_from_daemon(client: Option<Arc<DaemonClient>>, session: SessionId) {
    if let Some(client) = client {
        let unsubscribe = |id| Request::TerminalUnsubscribe {
            id,
            session_id: session,
        };
        let _ = client
            .request_within(UNSUBSCRIBE_TIMEOUT, unsubscribe)
            .await;
    }
}

struct Attached {
    client: Arc<DaemonClient>,
    output: mpsc::Receiver<Output>,
    events: broadcast::Receiver<Event>,
}

async fn stream(
    bridge: Bridge,
    session: SessionId,
    sink: Sink,
    started: oneshot::Sender<Result<(), BridgeError>>,
) {
    let mut started = Some(started);
    // End offset of the last byte handed to the sink.
    let mut delivered = None;
    loop {
        let mut attached = match attach(&bridge, session).await {
            Ok(attached) => attached,
            Err(error) => match started.take() {
                Some(started) => {
                    let _ = started.send(Err(error));
                    return;
                }
                // Resuming: wait out a daemon that is down, give up on anything else
                // (for example a restarted daemon that no longer knows the session).
                None if resumable(&error) => {
                    sleep(RETRY_PAUSE).await;
                    continue;
                }
                None => return,
            },
        };
        if let Some(started) = started.take() {
            let _ = started.send(Ok(()));
        }
        if !forward(&mut attached, session, &sink, &mut delivered).await {
            // The webview closed its channel: stop the daemon's stream too.
            unsubscribe_from_daemon(Some(attached.client), session).await;
            return;
        }
    }
}

/// Register for the session's output, then ask the daemon to stream it. Registering first
/// loses nothing: the daemon answers before it sends any output.
async fn attach(bridge: &Bridge, session: SessionId) -> Result<Attached, BridgeError> {
    let client = bridge.client().await?;
    // An earlier stream of this session may still be sending (this consumer fell behind, or
    // the stream was replaced). Its output all arrives before this reply, so once the reply
    // is in, nothing stale can reach the stream registered below.
    unsubscribe_from_daemon(Some(Arc::clone(&client)), session).await;
    let events = client.events()?;
    let output = client.subscribe_output(session)?;
    client
        .request(|id| Request::TerminalSubscribe {
            id,
            session_id: session,
        })
        .await?;
    Ok(Attached {
        client,
        output,
        events,
    })
}

fn resumable(error: &BridgeError) -> bool {
    matches!(
        error.code,
        Code::DaemonUnavailable | Code::DaemonStartFailed | Code::Disconnected | Code::Timeout
    )
}

/// Forward output until the stream must be re-established (true) or the sink is closed
/// (false).
async fn forward(
    attached: &mut Attached,
    session: SessionId,
    sink: &Sink,
    delivered: &mut Option<u64>,
) -> bool {
    loop {
        tokio::select! {
            chunk = attached.output.recv() => match chunk {
                Some(chunk) => if !deliver(sink, delivered, chunk) { return false },
                // The connection ended, or this consumer fell too far behind.
                None => return true,
            },
            event = attached.events.recv() => match event {
                Ok(Event::TerminalLagged { session_id }) if session_id == session => break,
                Ok(_) => {}
                // Closed: the connection ended. Lagged: our own lag notice may be lost.
                Err(_) => break,
            },
        }
    }
    // Whatever the daemon sent before it stopped is already queued: deliver it first.
    while let Ok(chunk) = attached.output.try_recv() {
        if !deliver(sink, delivered, chunk) {
            return false;
        }
    }
    true
}

fn deliver(sink: &Sink, delivered: &mut Option<u64>, chunk: Output) -> bool {
    fresh(delivered, chunk).is_none_or(sink)
}

/// The framed message for the part of `chunk` the sink has not seen, if any. A chunk that
/// starts after `delivered` means the daemon no longer holds the bytes in between; they are
/// not recoverable, so the gap shows up as a jump in `seq`.
fn fresh(delivered: &mut Option<u64>, chunk: Output) -> Option<Vec<u8>> {
    let seen = delivered.map_or(0, |end| end.saturating_sub(chunk.seq));
    let skip = usize::try_from(seen).unwrap_or(usize::MAX);
    if skip >= chunk.data.len() {
        return None;
    }
    *delivered = Some(chunk.seq + chunk.data.len() as u64);
    let mut frame = Vec::with_capacity(8 + chunk.data.len() - skip);
    frame.extend_from_slice(&(chunk.seq + skip as u64).to_be_bytes());
    frame.extend_from_slice(&chunk.data[skip..]);
    Some(frame)
}

#[tauri::command]
pub async fn terminal_subscribe(
    bridge: State<'_, Bridge>,
    id: SessionId,
    on_output: Channel,
) -> Result<(), BridgeError> {
    let sink: Sink = Box::new(move |frame| on_output.send(InvokeResponseBody::Raw(frame)).is_ok());
    bridge.terminal_subscribe(id, sink).await
}

/// Not `async` on purpose: Tauri runs it, in arrival order, on the thread that receives
/// webview messages, so the stop is queued before any later `terminal_subscribe` starts.
#[tauri::command]
pub fn terminal_unsubscribe(bridge: State<'_, Bridge>, id: SessionId) {
    bridge.terminal_unsubscribe(id);
}

#[cfg(test)]
mod tests;

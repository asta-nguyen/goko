//! Authenticated Unix-socket server.
//!
//! Conventions the clients (s1-09) rely on:
//!
//! - The first frame is a bare `Hello`; the reply is `HelloOk`. Wrong token or version
//!   gets an error, then the daemon closes the connection.
//! - Errors that cannot be tied to a request (bad hello, bad frame) are a `Response` with
//!   `id` 0 and an `error`, followed by a close, so clients must number requests from 1.
//! - Results: `session.create`, `session.get`, `session.stop` return the `SessionInfo`;
//!   `session.list` returns `{"sessions": [SessionInfo]}`; `terminal.subscribe` returns
//!   `{"replay_from", "next_seq"}`; every other request returns `{}`.
//! - After `events.subscribe` the connection receives `session.updated` for every state
//!   change and `session.exited` when a process ends.
//! - After `terminal.subscribe` the connection receives kind-1 frames: the retained output
//!   first (starting at `replay_from`), then live output, `seq` being the byte offset of
//!   each frame's first byte. A subscriber that falls behind gets `terminal.lagged` and
//!   must subscribe again.
//! - One writer task per connection puts responses, events, and output frames on the
//!   socket, so frames never interleave.

use crate::auth::Token;
use crate::paths::{remove_if_exists, Paths};
use crate::pty::{PtyError, PtyProcess, Snapshot, StreamItem};
use crate::session::{CreateSpec, SessionError, SessionManager};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protocol::frame::{self, Frame, FrameError, FrameKind, MAX_FRAME_PAYLOAD};
use protocol::message::{
    ErrorCode, Event, Hello, HelloOk, ProtoError, Request, Response, ResponseOutcome,
};
use protocol::session::{SessionId, SessionInfo, SessionState};
use protocol::PROTOCOL_VERSION;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, Permissions};
use std::future::Future;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, warn};

/// Encoded frames waiting for a connection's writer task.
const OUTGOING_QUEUE: usize = 64;
/// Retained output goes out in frames of at most this many bytes.
const SNAPSHOT_PIECE: usize = 64 * 1024;

struct State {
    token: Token,
    started: Instant,
    sessions: Arc<SessionManager>,
    worktree_operations_lock: PathBuf,
}

/// Replace any stale `daemon.sock` and bind a fresh one with mode 0600. The caller must
/// hold the instance lock, which proves no live daemon owns the old socket.
pub fn bind(paths: &Paths) -> io::Result<UnixListener> {
    remove_if_exists(&paths.socket())?;
    let listener = UnixListener::bind(paths.socket())?;
    fs::set_permissions(paths.socket(), Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accept clients until `shutdown` completes. Open connections are dropped on return;
/// the sessions keep running until the caller shuts the manager down.
pub async fn serve(
    listener: UnixListener,
    token: Token,
    sessions: Arc<SessionManager>,
    worktree_operations_lock: PathBuf,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let state = Arc::new(State {
        token,
        started: Instant::now(),
        sessions,
        worktree_operations_lock,
    });
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(handle(stream, Arc::clone(&state)));
                }
                Err(error) => {
                    warn!("could not accept a connection: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

async fn acquire_session_launch_lock(path: PathBuf) -> Result<File, ProtoError> {
    let file = tokio::task::spawn_blocking(move || -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        // SAFETY: `file` stays open while the shared lock protects PTY creation.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    })
    .await
    .map_err(|error| ProtoError {
        code: ErrorCode::Internal,
        message: format!("cannot acquire the session launch lock: {error}"),
        hint: None,
    })?
    .map_err(|error| ProtoError {
        code: ErrorCode::Internal,
        message: format!("cannot acquire the session launch lock: {error}"),
        hint: None,
    })?;
    Ok(file)
}

async fn handle(stream: UnixStream, state: Arc<State>) {
    if let Err(error) = connection(stream, &state).await {
        match error.kind() {
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset => {
                debug!("client went away: {error}");
            }
            _ => warn!("connection failed: {error}"),
        }
    }
}

async fn connection(mut stream: UnixStream, state: &State) -> io::Result<()> {
    let payload = match next_control(&mut stream).await? {
        Next::Payload(payload) => payload,
        Next::Finished(reply) => return send_final(&mut stream, reply).await,
    };
    let hello = match serde_json::from_slice::<Hello>(&payload) {
        Ok(hello) => hello,
        Err(error) => {
            let message = format!("the first frame must be a hello: {error}");
            let reply = error_response(0, ErrorCode::Malformed, message);
            return send_final(&mut stream, Some(reply)).await;
        }
    };
    if !state.token.verify(&hello.token) {
        let reply = error_response(0, ErrorCode::AuthFailed, "wrong token");
        return send_final(&mut stream, Some(reply)).await;
    }
    if hello.protocol_version != PROTOCOL_VERSION {
        let message = format!(
            "protocol version {} is not supported; this daemon speaks {PROTOCOL_VERSION}",
            hello.protocol_version
        );
        let reply = error_response(0, ErrorCode::VersionUnsupported, message);
        return send_final(&mut stream, Some(reply)).await;
    }
    let hello_ok = HelloOk {
        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    stream.write_all(&encode_control(&hello_ok)?).await?;

    let (mut reader, writer) = stream.into_split();
    let (outgoing, queued) = mpsc::channel(OUTGOING_QUEUE);
    let writer = tokio::spawn(write_frames(writer, queued));
    let mut client = Client {
        state,
        outgoing,
        events: None,
        terminals: HashMap::new(),
    };
    let served = client.serve(&mut reader).await;
    // Stop the subscription tasks and drop our sender so the writer flushes what is
    // queued (including a final error) and ends.
    drop(client);
    let written = writer.await.map_err(io::Error::other)?;
    served.and(written)
}

/// One authenticated connection: its outgoing queue and its subscriptions.
struct Client<'a> {
    state: &'a State,
    outgoing: mpsc::Sender<Vec<u8>>,
    events: Option<JoinHandle<()>>,
    terminals: HashMap<SessionId, JoinHandle<()>>,
}

impl Drop for Client<'_> {
    /// A disconnect ends the subscriptions and never touches a session.
    fn drop(&mut self) {
        for task in self.events.iter().chain(self.terminals.values()) {
            task.abort();
        }
    }
}

/// A `terminal.subscribe` whose output has not started streaming yet.
struct Output {
    session_id: SessionId,
    snapshot: Snapshot,
    receiver: mpsc::Receiver<StreamItem>,
}

impl Client<'_> {
    async fn serve(&mut self, reader: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
        loop {
            match next_control(reader).await? {
                Next::Payload(payload) => self.handle(&payload).await?,
                Next::Finished(reply) => {
                    if let Some(reply) = reply {
                        self.send(&reply).await?;
                    }
                    return Ok(());
                }
            }
        }
    }

    async fn handle(&mut self, payload: &[u8]) -> io::Result<()> {
        let value = match serde_json::from_slice::<Value>(payload) {
            Ok(value) => value,
            Err(error) => {
                let message = format!("invalid JSON: {error}");
                return self
                    .send(&error_response(0, ErrorCode::Malformed, message))
                    .await;
            }
        };
        let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
        let request = match serde_json::from_value::<Request>(value) {
            Ok(request) => request,
            Err(error) => {
                let message = format!("invalid request: {error}");
                return self
                    .send(&error_response(id, ErrorCode::Malformed, message))
                    .await;
            }
        };

        let (outcome, output) = self.dispatch(request).await;
        let outcome = match outcome {
            Ok(result) => ResponseOutcome::Result { result },
            Err(error) => ResponseOutcome::Error { error },
        };
        self.send(&Response { id, outcome }).await?;
        // The response goes first: the client learns `next_seq` before any frame arrives.
        if let Some(output) = output {
            let session_id = output.session_id;
            let task = tokio::spawn(stream_output(self.outgoing.clone(), output));
            self.terminals.insert(session_id, task);
        }
        Ok(())
    }

    async fn dispatch(&mut self, request: Request) -> (Result<Value, ProtoError>, Option<Output>) {
        let sessions = &self.state.sessions;
        let outcome = match request {
            Request::SystemHealth { .. } => Ok(json!({
                "status": "ok",
                "uptime_seconds": self.state.started.elapsed().as_secs(),
            })),
            Request::SystemVersion { .. } => Ok(json!({
                "daemon_version": env!("CARGO_PKG_VERSION"),
                "protocol_version": PROTOCOL_VERSION,
            })),
            Request::EventsSubscribe { .. } => {
                self.subscribe_events();
                Ok(json!({}))
            }
            Request::SessionCreate {
                argv,
                cwd,
                cols,
                rows,
                title,
                ..
            } => match acquire_session_launch_lock(self.state.worktree_operations_lock.clone())
                .await
            {
                Ok(_lock) => sessions
                    .create(CreateSpec {
                        argv,
                        cwd,
                        cols,
                        rows,
                        title,
                    })
                    .map(|info| json!(info))
                    .map_err(session_error),
                Err(error) => Err(error),
            },
            Request::SessionList { .. } => Ok(json!({ "sessions": sessions.list() })),
            Request::SessionGet { session_id, .. } => sessions
                .get(session_id)
                .map(|info| json!(info))
                .map_err(session_error),
            Request::SessionStop {
                session_id, force, ..
            } => sessions
                .stop(session_id, force)
                .map(|info| json!(info))
                .map_err(session_error),
            Request::SessionDelete { session_id, .. } => {
                let deleted = sessions
                    .delete(session_id)
                    .map(|()| json!({}))
                    .map_err(session_error);
                if deleted.is_ok() {
                    if let Some(task) = self.terminals.remove(&session_id) {
                        task.abort();
                        let _ = task.await;
                    }
                }
                deleted
            }
            Request::TerminalWrite {
                session_id,
                data_b64,
                ..
            } => self.terminal_write(session_id, &data_b64).await,
            Request::TerminalResize {
                session_id,
                cols,
                rows,
                ..
            } => self.live_pty(session_id).and_then(|pty| {
                pty.resize(cols, rows).map_err(pty_error)?;
                Ok(json!({}))
            }),
            Request::TerminalSubscribe { session_id, .. } => {
                return match self.terminal_subscribe(session_id) {
                    Ok((result, output)) => (Ok(result), Some(output)),
                    Err(error) => (Err(error), None),
                };
            }
            Request::TerminalUnsubscribe { session_id, .. } => {
                if let Some(task) = self.terminals.remove(&session_id) {
                    // Wait for the task to stop so no frame follows the response.
                    task.abort();
                    let _ = task.await;
                }
                Ok(json!({}))
            }
        };
        (outcome, None)
    }

    fn subscribe_events(&mut self) {
        if self.events.is_none() {
            let receiver = self.state.sessions.subscribe_events();
            let sessions = Arc::clone(&self.state.sessions);
            self.events = Some(tokio::spawn(forward_events(
                self.outgoing.clone(),
                sessions,
                receiver,
            )));
        }
    }

    async fn terminal_write(
        &self,
        session_id: SessionId,
        data_b64: &str,
    ) -> Result<Value, ProtoError> {
        let bytes = STANDARD.decode(data_b64).map_err(|error| {
            proto_error(
                ErrorCode::Malformed,
                format!("data_b64 is not valid base64: {error}"),
            )
        })?;
        let pty = self.live_pty(session_id)?;
        // Writing can block while the child is not reading its input.
        tokio::task::spawn_blocking(move || pty.write(&bytes))
            .await
            .map_err(|error| proto_error(ErrorCode::Internal, error.to_string()))?
            .map_err(pty_error)?;
        Ok(json!({}))
    }

    fn terminal_subscribe(&mut self, session_id: SessionId) -> Result<(Value, Output), ProtoError> {
        let pty = self.state.sessions.pty(session_id).map_err(session_error)?;
        let (snapshot, receiver) = pty.subscribe();
        // A repeat subscribe (after `terminal.lagged`) replaces the old stream.
        if let Some(old) = self.terminals.remove(&session_id) {
            old.abort();
        }
        let result = json!({
            "replay_from": snapshot.replay_from,
            "next_seq": snapshot.next_seq,
        });
        let output = Output {
            session_id,
            snapshot,
            receiver,
        };
        Ok((result, output))
    }

    /// The process of a session that is still running, for input and resizing.
    fn live_pty(&self, session_id: SessionId) -> Result<Arc<PtyProcess>, ProtoError> {
        let info = self.state.sessions.get(session_id).map_err(session_error)?;
        if matches!(info.state, SessionState::Exited | SessionState::Failed) {
            return Err(proto_error(
                ErrorCode::InvalidArgument,
                "the session has exited",
            ));
        }
        self.state.sessions.pty(session_id).map_err(session_error)
    }

    async fn send(&self, message: &impl Serialize) -> io::Result<()> {
        send_queued(&self.outgoing, encode_control(message)?).await
    }
}

/// Push every session state change to one client; if it falls too far behind, send the
/// current state of every session instead of the missed changes.
async fn forward_events(
    outgoing: mpsc::Sender<Vec<u8>>,
    sessions: Arc<SessionManager>,
    mut events: broadcast::Receiver<SessionInfo>,
) {
    loop {
        let changed = match events.recv().await {
            Ok(info) => vec![info],
            Err(broadcast::error::RecvError::Lagged(_)) => sessions.list(),
            Err(broadcast::error::RecvError::Closed) => return,
        };
        for info in changed {
            let exited = (info.state == SessionState::Exited).then_some(Event::SessionExited {
                session_id: info.id,
                exit_code: info.exit_code,
            });
            let updated = Event::SessionUpdated { session: info };
            for event in std::iter::once(updated).chain(exited) {
                let Ok(frame) = encode_control(&event) else {
                    return;
                };
                if send_queued(&outgoing, frame).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Send the retained output, then live output, as kind-1 frames until the stream ends.
async fn stream_output(outgoing: mpsc::Sender<Vec<u8>>, mut output: Output) {
    let id = *output.session_id.as_bytes();
    let snapshot = &output.snapshot;
    for (index, piece) in snapshot.bytes.chunks(SNAPSHOT_PIECE).enumerate() {
        let seq = snapshot.replay_from + (index * SNAPSHOT_PIECE) as u64;
        if send_output(&outgoing, id, seq, piece).await.is_err() {
            return;
        }
    }

    while let Some(item) = output.receiver.recv().await {
        match item {
            StreamItem::Chunk { seq, bytes } => {
                if send_output(&outgoing, id, seq, &bytes).await.is_err() {
                    return;
                }
            }
            StreamItem::Lagged => {
                let lagged = Event::TerminalLagged {
                    session_id: output.session_id,
                };
                if let Ok(frame) = encode_control(&lagged) {
                    let _ = send_queued(&outgoing, frame).await;
                }
                return;
            }
            StreamItem::Closed => return,
        }
    }
}

async fn send_output(
    outgoing: &mpsc::Sender<Vec<u8>>,
    session_id: [u8; 16],
    seq: u64,
    data: &[u8],
) -> io::Result<()> {
    let payload = frame::encode_output(session_id, seq, data).map_err(io::Error::other)?;
    let bytes = frame::encode(&Frame {
        kind: FrameKind::TerminalOutput,
        payload,
    })
    .map_err(io::Error::other)?;
    send_queued(outgoing, bytes).await
}

async fn send_queued(outgoing: &mpsc::Sender<Vec<u8>>, bytes: Vec<u8>) -> io::Result<()> {
    outgoing
        .send(bytes)
        .await
        .map_err(|_| io::ErrorKind::BrokenPipe.into())
}

/// The connection's only writer: frames reach the socket whole and in queue order.
async fn write_frames(
    mut writer: OwnedWriteHalf,
    mut queued: mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    while let Some(bytes) = queued.recv().await {
        writer.write_all(&bytes).await?;
    }
    Ok(())
}

fn session_error(error: SessionError) -> ProtoError {
    let code = match error {
        SessionError::InvalidArgument(_) => ErrorCode::InvalidArgument,
        SessionError::NotFound(_) => ErrorCode::NotFound,
    };
    proto_error(code, error.to_string())
}

fn pty_error(error: PtyError) -> ProtoError {
    let code = match error {
        PtyError::InvalidArgument(_) => ErrorCode::InvalidArgument,
        PtyError::SpawnFailed(_) | PtyError::Io(_) => ErrorCode::Internal,
    };
    proto_error(code, error.to_string())
}

fn proto_error(code: ErrorCode, message: impl Into<String>) -> ProtoError {
    ProtoError {
        code,
        message: message.into(),
        hint: None,
    }
}

fn error_response(id: u64, code: ErrorCode, message: impl Into<String>) -> Response {
    Response {
        id,
        outcome: ResponseOutcome::Error {
            error: proto_error(code, message),
        },
    }
}

/// What the next frame from a client means.
enum Next {
    Payload(Vec<u8>),
    /// The connection is over; send the error first if there is one.
    Finished(Option<Response>),
}

/// The next control payload, or why the connection is over.
async fn next_control(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Next> {
    let error = match read_frame(reader).await? {
        Ok(Some(Frame {
            kind: FrameKind::Control,
            payload,
        })) => return Ok(Next::Payload(payload)),
        Ok(Some(_)) => FrameError::Malformed("clients may only send control frames"),
        Ok(None) => return Ok(Next::Finished(None)),
        Err(error) => error,
    };
    let code = match error {
        FrameError::FrameTooLarge(_) => ErrorCode::FrameTooLarge,
        _ => ErrorCode::Malformed,
    };
    Ok(Next::Finished(Some(error_response(
        0,
        code,
        error.to_string(),
    ))))
}

/// Read one frame. `Ok(None)` means the peer closed the connection, even mid-frame; the
/// inner `Err` is a frame the client must not send.
async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
) -> io::Result<Result<Option<Frame>, FrameError>> {
    let mut header = [0_u8; 5];
    if !read_exact(reader, &mut header).await? {
        return Ok(Ok(None));
    }
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Ok(Err(FrameError::FrameTooLarge(length)));
    }

    let mut bytes = vec![0_u8; header.len() + length];
    bytes[..header.len()].copy_from_slice(&header);
    if !read_exact(reader, &mut bytes[header.len()..]).await? {
        return Ok(Ok(None));
    }
    Ok(frame::decode(&bytes).map(|(frame, _)| Some(frame)))
}

/// `Ok(false)` when the peer closed before `buffer` was full.
async fn read_exact(reader: &mut (impl AsyncRead + Unpin), buffer: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(buffer).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn encode_control(message: &impl Serialize) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    frame::encode(&Frame {
        kind: FrameKind::Control,
        payload,
    })
    .map_err(io::Error::other)
}

/// Before the writer task exists: send the closing error, if any.
async fn send_final(
    stream: &mut (impl AsyncWrite + Unpin),
    reply: Option<Response>,
) -> io::Result<()> {
    match reply {
        Some(reply) => stream.write_all(&encode_control(&reply)?).await,
        None => Ok(()),
    }
}

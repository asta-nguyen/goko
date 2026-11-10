//! gokod client on an owned Tokio runtime thread — the Rune port of the
//! desktop's `daemon_client.rs`.
//!
//! A supervisor loop keeps one authenticated connection alive: it connects
//! (spawning `gokod` when the socket is absent), sends `Hello`/`HelloOk`,
//! re-issues `events.subscribe` and `session.list` after every handshake,
//! and redials with a capped backoff whenever the socket drops. While the
//! connection is up, a reader task demultiplexes incoming frames: responses
//! complete the request registered under their `id`, events surface as
//! [`UiEvent`]s, and kind-1 output frames route to the subscriber that
//! registered for the session. A writer task serialises outgoing frames, so
//! a request that times out can never leave half a frame on the wire.
//!
//! The UI thread never touches the socket and never blocks on it:
//! [`Daemon::call`] hands back a channel whose reply is drained from `poll`
//! (10 s cap, enforced runtime-side), [`Daemon::subscribe_output`] hands back
//! a bounded per-session channel the same way, and events flow through
//! [`Daemon::take_events`], refilled from the runtime thread which wakes the
//! event loop after every push. Dropping the client's side of the command
//! channel — [`Daemon::shutdown`] or `Drop` — ends the supervisor.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use app_core::error::{BridgeError, Code};
use protocol::PROTOCOL_VERSION;
use protocol::frame::{self, Frame, FrameKind, MAX_FRAME_PAYLOAD};
use protocol::message::{ErrorCode, Event, Hello, HelloOk, Request, Response, ResponseOutcome};
use protocol::session::{SessionId, SessionInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc as tmpsc;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

/// Cap on every request's flight time, enforced runtime-side; the drained
/// reply carries the timeout as an error, so `poll` never waits on one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Pauses between redial attempts once the socket is gone, capped at the
/// last entry so a long outage keeps probing.
const REDIAL_BACKOFF_MS: [u64; 7] = [50, 100, 200, 400, 800, 1_600, 3_200];
/// Client-side lag: an output subscriber more than this many chunks behind
/// is dropped (the session must resubscribe to recover).
const OUTPUT_QUEUE: usize = 256;
/// Outgoing frames are client-paced and small; 64 keeps a wedged writer
/// from buffering an unbounded queue.
const WRITE_QUEUE: usize = 64;

/// One chunk of terminal output; `seq` is the PTY byte offset of `data[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Everything the runtime thread wants `State` to know, drained in `poll`.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// The handshake completed; the dot goes green and the version is kept
    /// for capability flags.
    Connected { version: String },
    /// The socket dropped or the connect attempt failed (the reason is the
    /// message; the pill shows while no connection is up).
    Disconnected { reason: String },
    /// The authoritative session list after a (re)connect: replaces the
    /// rows, reconciling anything missed while disconnected.
    Sessions(Vec<SessionInfo>),
    /// A daemon event (`session.updated`, `session.exited`,
    /// `terminal.lagged`).
    Event(Event),
}

/// The daemon's `session.list` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionInfo>,
}

/// The daemon's `terminal.subscribe` payload.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SubscribeReply {
    /// The first byte the replay carries; earlier bytes are gone. The app
    /// does not mark the gap yet.
    #[allow(dead_code)]
    pub replay_from: u64,
    /// The next byte the live stream will carry.
    pub next_seq: u64,
}

/// The outcome of one request.
type Outcome = Result<Response, BridgeError>;

/// An attached terminal: the session's output chunks and the daemon's
/// answer to the subscribe request.
pub type Attached = (
    mpsc::Receiver<Output>,
    mpsc::Receiver<Result<Response, BridgeError>>,
);

/// A pending request's reply slot. Exactly one caller resolves it: the
/// response, the timeout, or the disconnect — whichever comes first.
type Resolver = Arc<Mutex<Option<Box<dyn FnOnce(Outcome) + Send>>>>;

/// Resolve a pending request. A second caller finds the slot empty and
/// does nothing.
fn resolve(resolver: &Resolver, outcome: Outcome) {
    let taken = resolver.lock().unwrap().take();
    if let Some(finish) = taken {
        finish(outcome);
    }
}

/// What the runtime thread should do next.
enum Cmd {
    /// Send one request. The builder runs on the runtime thread so the id
    /// it stamps is the authoritative one.
    Call {
        build: Box<dyn FnOnce(u64) -> Request + Send>,
        reply: Resolver,
    },
    /// Route kind-1 frames for `session` to `tx`, replacing any earlier
    /// subscriber.
    SubscribeOutput {
        session: SessionId,
        tx: SyncSender<Output>,
    },
    /// Stop routing frames for `session`.
    UnsubscribeOutput { session: SessionId },
}

/// Routes for a live connection: pending requests and per-session output
/// subscribers. `None` once the connection is gone; dropping the inner
/// senders ends every waiter.
type Shared = Arc<Mutex<Option<Routes>>>;

struct Routes {
    pending: HashMap<u64, Resolver>,
    outputs: HashMap<SessionId, SyncSender<Output>>,
}

/// The wake callback the runtime thread calls after every event push.
type Wake = Arc<dyn Fn() + Send + Sync>;

/// UI-side handle: command sender, drained-event receiver, join handle.
pub struct Daemon {
    tx: Option<tmpsc::UnboundedSender<Cmd>>,
    events: mpsc::Receiver<UiEvent>,
    /// The version the last successful handshake reported, for capability
    /// flags (`supports_flow_control`).
    version: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Daemon {
    /// Start the runtime thread. `wake` runs after every event push so the
    /// app's `poll` drains it promptly.
    ///
    /// The tests never spawn a daemon of their own — they drive the
    /// client against a fake or a real one they started themselves — so
    /// this entry point is unused there.
    #[cfg_attr(test, allow(dead_code))]
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Daemon {
        let home = app_core::home::home_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::spawn_runtime(home, Mode::Spawn, wake)
    }

    /// Test constructor: connect to whatever listens at `home`, never
    /// spawning `gokod` (the test owns the socket).
    #[cfg(test)]
    pub(crate) fn spawn_connect_only(
        home: PathBuf,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Daemon {
        Self::spawn_runtime(home, Mode::ConnectOnly, wake)
    }

    /// Test constructor: a client that never reaches a daemon. Every
    /// request fails with `Disconnected` at once, so no test can touch a
    /// real socket or process.
    #[cfg(test)]
    pub(crate) fn spawn_disconnected() -> Daemon {
        Self::spawn_runtime(PathBuf::from("/nonexistent"), Mode::Disabled, || {})
    }

    fn spawn_runtime(home: PathBuf, mode: Mode, wake: impl Fn() + Send + Sync + 'static) -> Daemon {
        let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
        let version = Arc::new(Mutex::new(None));
        let thread = std::thread::Builder::new()
            .name("goko-daemon".into())
            .spawn({
                let version = Arc::clone(&version);
                move || runtime_main(home, mode, ready_tx, wake, version)
            })
            .expect("spawn goko-daemon runtime");
        let ready = ready_rx.recv().expect("daemon runtime sends its channels");
        Daemon {
            tx: Some(ready.tx),
            events: ready.events,
            version,
            thread: Some(thread),
        }
    }

    /// Drain the events the runtime queued since the last call, in arrival
    /// order. The runtime keeps pushing while the consumer catches up, so
    /// no event is lost: the connect-time list refresh reconciles
    /// everything after every reconnect.
    pub fn take_events(&self) -> Vec<UiEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    /// Send one request and hand back the channel its reply resolves
    /// through. The builder runs on the runtime thread and receives the
    /// authoritative request id. `None` only when the runtime is gone.
    pub fn call(
        &self,
        build: impl FnOnce(u64) -> Request + Send + 'static,
    ) -> Option<mpsc::Receiver<Result<Response, BridgeError>>> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let resolver: Resolver = Arc::new(Mutex::new(Some(Box::new(move |outcome| {
            let _ = reply_tx.send(outcome);
        }))));
        if !self.send(Cmd::Call {
            build: Box::new(build),
            reply: resolver,
        }) {
            return None;
        }
        Some(reply_rx)
    }

    /// Send one request and resolve its `result` payload into `T`. The
    /// parse runs on the runtime thread, so the UI only ever sees the
    /// typed value.
    pub fn call_json<T: DeserializeOwned + Send + 'static>(
        &self,
        build: impl FnOnce(u64) -> Request + Send + 'static,
    ) -> Option<mpsc::Receiver<Result<T, BridgeError>>> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let resolver: Resolver = Arc::new(Mutex::new(Some(Box::new(move |outcome| {
            let parsed = match outcome {
                Ok(response) => match result_of(&response) {
                    Ok(value) => serde_json::from_value(value.clone()).map_err(|error| {
                        BridgeError::new(
                            Code::Daemon(ErrorCode::Malformed),
                            format!("the daemon's answer did not fit the expected shape: {error}"),
                        )
                    }),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            let _ = reply_tx.send(parsed);
        }))));
        if !self.send(Cmd::Call {
            build: Box::new(build),
            reply: resolver,
        }) {
            return None;
        }
        Some(reply_rx)
    }

    /// Register `session`'s output subscriber, replacing any earlier one.
    /// Register before sending `terminal.subscribe` so no chunk is missed.
    /// The receiver ends when the connection drops or the consumer falls
    /// [`OUTPUT_QUEUE`] chunks behind.
    pub fn subscribe_output(&self, session: SessionId) -> Option<mpsc::Receiver<Output>> {
        let (tx, rx) = mpsc::sync_channel(OUTPUT_QUEUE);
        if !self.send(Cmd::SubscribeOutput { session, tx }) {
            return None;
        }
        Some(rx)
    }

    /// Drop `session`'s local subscriber so no more chunks route to it.
    pub fn unsubscribe_output(&self, session: SessionId) {
        self.send(Cmd::UnsubscribeOutput { session });
    }

    /// Attach to a session's terminal: register the output route first,
    /// then ask the daemon to stream it. The first channel carries the
    /// output chunks, the second the daemon's `terminal.subscribe` reply.
    pub fn attach_output(&self, session: SessionId, flow: bool) -> Option<Attached> {
        let chunks = self.subscribe_output(session)?;
        let ready = self.call(move |id| Request::TerminalSubscribe {
            id,
            session_id: session,
            flow,
        })?;
        Some((chunks, ready))
    }

    /// Detach from a session's terminal: stop routing chunks locally and
    /// tell the daemon to stop the stream.
    pub fn detach_output(&self, session: SessionId) {
        self.unsubscribe_output(session);
        let _ = self.call(move |id| Request::TerminalUnsubscribe {
            id,
            session_id: session,
        });
    }

    /// The version the last successful handshake reported.
    pub fn daemon_version(&self) -> Option<String> {
        self.version.lock().unwrap().clone()
    }

    /// True when the daemon is new enough to keep the terminal stream
    /// flowing (it acknowledges the bytes it has sent).
    pub fn supports_flow_control(&self) -> bool {
        self.daemon_version()
            .is_some_and(|version| version_at_least(&version, (0, 1, 2)))
    }

    fn send(&self, cmd: Cmd) -> bool {
        match &self.tx {
            Some(tx) if !tx.is_closed() => tx.send(cmd).is_ok(),
            _ => false,
        }
    }

    /// App exit: close the command channel, then wait for the runtime
    /// thread to finish its teardown (dropping pending replies and every
    /// output subscription). Blocks briefly; called from `on_quit`.
    pub fn shutdown(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The daemon's `result` payload, or the daemon's error as a `BridgeError`.
pub fn result_of(response: &Response) -> Result<&serde_json::Value, BridgeError> {
    match &response.outcome {
        ResponseOutcome::Result { result } => Ok(result),
        ResponseOutcome::Error { error } => Err(error.clone().into()),
    }
}

/// The channels the runtime thread hands to the UI side.
struct Ready {
    tx: tmpsc::UnboundedSender<Cmd>,
    events: mpsc::Receiver<UiEvent>,
}

/// Whether the runtime may start `gokod` when the socket is absent.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // ConnectOnly and Disabled are the test modes.
enum Mode {
    /// Production: start `gokod` when the socket is absent.
    Spawn,
    /// Tests: never start a process; the test owns the socket.
    ConnectOnly,
    /// Tests: no daemon at all; every request fails at once.
    Disabled,
}

/// Why the serve loop ended.
enum ServeEnd {
    /// The UI dropped the command channel: the app is quitting.
    UiGone,
    /// The connection died: reconnect.
    ConnectionLost,
}

fn runtime_main(
    home: PathBuf,
    mode: Mode,
    ready: mpsc::Sender<Ready>,
    wake: impl Fn() + Send + Sync + 'static,
    version: Arc<Mutex<Option<String>>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };
    runtime.block_on(async move {
        let (tx, mut rx) = tmpsc::unbounded_channel::<Cmd>();
        let (event_tx, event_rx) = mpsc::channel::<UiEvent>();
        let _ = ready.send(Ready {
            tx,
            events: event_rx,
        });
        supervise(home, mode, &mut rx, &event_tx, Arc::new(wake), version).await;
    });
}

/// Keep one connection alive: connect, bootstrap, serve, repeat.
async fn supervise(
    home: PathBuf,
    mode: Mode,
    cmd_rx: &mut tmpsc::UnboundedReceiver<Cmd>,
    event_tx: &mpsc::Sender<UiEvent>,
    wake: Wake,
    version: Arc<Mutex<Option<String>>>,
) {
    let mut attempt = 0_usize;
    let mut spawned = false;
    if mode == Mode::Disabled {
        // No daemon: every request fails at once and the client waits for
        // the UI to go away.
        while !cmd_rx.is_closed() {
            fail_queued(cmd_rx);
            sleep(Duration::from_millis(1)).await;
        }
        return;
    }
    loop {
        // The UI dropped the command channel: the app is quitting.
        if cmd_rx.is_closed() {
            return;
        }
        let mut reason = "the connection to gokod was lost".to_string();
        match connect(&home, event_tx, &wake).await {
            Ok(mut conn) => {
                attempt = 0;
                spawned = false;
                *version.lock().unwrap() = Some(conn.version.clone());
                push_event(
                    event_tx,
                    &wake,
                    UiEvent::Connected {
                        version: conn.version.clone(),
                    },
                );
                match bootstrap(&mut conn, event_tx, &wake).await {
                    Ok(()) => match serve(&mut conn, cmd_rx).await {
                        ServeEnd::UiGone => return,
                        ServeEnd::ConnectionLost => {}
                    },
                    Err(error) => reason = error.message,
                }
            }
            Err(error) => {
                reason = error.message;
                // One spawn attempt per outage: the daemon writes its own
                // socket, and a second process would only lose the race.
                if mode == Mode::Spawn && !spawned {
                    spawned = true;
                    match daemon_bin().and_then(|bin| spawn_daemon(&home, &bin)) {
                        Ok(()) => {}
                        Err(spawn_error) => reason = spawn_error.message,
                    }
                }
            }
        }
        *version.lock().unwrap() = None;
        push_event(event_tx, &wake, UiEvent::Disconnected { reason });
        // Whatever the UI asked for while the connection was down is
        // answered now instead of silently queueing for the next one.
        fail_queued(cmd_rx);
        if cmd_rx.is_closed() {
            return;
        }
        let pause = REDIAL_BACKOFF_MS[attempt.min(REDIAL_BACKOFF_MS.len() - 1)];
        attempt = attempt.saturating_add(1);
        // Wait out the backoff, but stop at once when the UI goes away —
        // the app must not wait for the supervisor to notice.
        tokio::select! {
            _ = sleep(Duration::from_millis(pause)) => {}
            cmd = cmd_rx.recv() => match cmd {
                None => return,
                Some(cmd) => fail_cmd(cmd),
            }
        }
    }
}

/// Answer one request that arrived while the daemon was unreachable.
fn fail_cmd(cmd: Cmd) {
    if let Cmd::Call { reply, .. } = cmd {
        resolve(&reply, Err(disconnected()));
    }
}

/// Subscribe to the daemon's events and read the authoritative session
/// list. Both must succeed: a daemon that cannot answer them is not
/// usable, and the supervisor retries.
async fn bootstrap(
    conn: &mut Connection,
    event_tx: &mpsc::Sender<UiEvent>,
    wake: &Wake,
) -> Result<(), BridgeError> {
    let response = conn.request(|id| Request::EventsSubscribe { id }).await?;
    result_of(&response)?;
    let response = conn.request(|id| Request::SessionList { id }).await?;
    let list: SessionList =
        serde_json::from_value(result_of(&response)?.clone()).map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Malformed),
                format!("the daemon's session list did not fit the expected shape: {error}"),
            )
        })?;
    push_event(event_tx, wake, UiEvent::Sessions(list.sessions));
    Ok(())
}

/// Run the command loop until the connection dies or the UI goes away.
async fn serve(conn: &mut Connection, cmd_rx: &mut tmpsc::UnboundedReceiver<Cmd>) -> ServeEnd {
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(cmd) => handle_cmd(conn, cmd).await,
                // The UI dropped the command channel: the app is quitting.
                None => return ServeEnd::UiGone,
            },
            // The reader task ended: the connection is gone.
            _ = &mut conn.closed => return ServeEnd::ConnectionLost,
        }
    }
}

/// Apply one command from the UI.
async fn handle_cmd(conn: &mut Connection, cmd: Cmd) {
    match cmd {
        Cmd::Call { build, reply } => {
            let id = conn.next_id.fetch_add(1, Ordering::Relaxed);
            let request = build(id);
            conn.send_request(id, request, reply).await;
        }
        Cmd::SubscribeOutput { session, tx } => {
            let mut guard = conn.shared.lock().unwrap();
            if let Some(routes) = guard.as_mut() {
                routes.outputs.insert(session, tx);
            }
        }
        Cmd::UnsubscribeOutput { session } => {
            let mut guard = conn.shared.lock().unwrap();
            if let Some(routes) = guard.as_mut() {
                routes.outputs.remove(&session);
            }
        }
    }
}

/// Answer every request the UI queued while the connection was down.
fn fail_queued(cmd_rx: &mut tmpsc::UnboundedReceiver<Cmd>) {
    while let Ok(cmd) = cmd_rx.try_recv() {
        match cmd {
            Cmd::Call { reply, .. } => resolve(&reply, Err(disconnected())),
            Cmd::SubscribeOutput { .. } | Cmd::UnsubscribeOutput { .. } => {}
        }
    }
}

/// A live connection to the daemon.
struct Connection {
    shared: Shared,
    writes: tmpsc::Sender<Vec<u8>>,
    /// Resolves when the reader task ends: the connection is gone.
    closed: oneshot::Receiver<()>,
    version: String,
    /// Request ids only need to be unique within one connection.
    next_id: AtomicU64,
}

impl Connection {
    /// Send one request and resolve its response.
    async fn request(&self, build: impl FnOnce(u64) -> Request) -> Result<Response, BridgeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = build(id);
        let bytes = encode_control(&request)?;
        let (answer, answers) = oneshot::channel();
        let resolver: Resolver = Arc::new(Mutex::new(Some(Box::new(move |outcome| {
            let _ = answer.send(outcome);
        }))));
        {
            let mut guard = self.shared.lock().unwrap();
            let Some(routes) = guard.as_mut() else {
                return Err(disconnected());
            };
            routes.pending.insert(id, resolver);
        }
        if self.writes.send(bytes).await.is_err() {
            remove_pending(&self.shared, id);
            return Err(disconnected());
        }
        match timeout(REQUEST_TIMEOUT, answers).await {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => Err(timed_out()),
        }
    }

    /// Register the reply route, put the frame on the wire and arm the
    /// timeout that ends a request the daemon never answers.
    async fn send_request(&self, id: u64, request: Request, reply: Resolver) {
        let bytes = match encode_control(&request) {
            Ok(bytes) => bytes,
            Err(error) => {
                resolve(&reply, Err(error));
                return;
            }
        };
        {
            let mut guard = self.shared.lock().unwrap();
            let Some(routes) = guard.as_mut() else {
                resolve(&reply, Err(disconnected()));
                return;
            };
            routes.pending.insert(id, Arc::clone(&reply));
        }
        // The watchdog ends the request even when the daemon stays silent:
        // the UI must never wait on a reply that never comes.
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            sleep(REQUEST_TIMEOUT).await;
            if let Some(pending) = remove_pending(&shared, id) {
                resolve(&pending, Err(timed_out()));
            }
        });
        if self.writes.send(bytes).await.is_err()
            && let Some(pending) = remove_pending(&self.shared, id)
        {
            resolve(&pending, Err(disconnected()));
        }
    }
}

/// Take the pending reply for `id`, if it is still waiting.
fn remove_pending(shared: &Shared, id: u64) -> Option<Resolver> {
    let mut guard = shared.lock().unwrap();
    guard.as_mut().and_then(|routes| routes.pending.remove(&id))
}

/// Connect to the daemon in `home` and authenticate.
async fn connect(
    home: &Path,
    event_tx: &mpsc::Sender<UiEvent>,
    wake: &Wake,
) -> Result<Connection, BridgeError> {
    let mut stream = UnixStream::connect(home.join("daemon.sock"))
        .await
        .map_err(unavailable)?;
    // The token is read only once the socket accepted: a live daemon
    // writes it before binding, so a stale file from a crashed daemon is
    // never sent.
    let token = fs::read_to_string(home.join("daemon.token")).map_err(unavailable)?;
    let daemon_version = timeout(REQUEST_TIMEOUT, handshake(&mut stream, token))
        .await
        .map_err(|_| timed_out())??;

    let (reader, mut writer) = stream.into_split();
    let shared: Shared = Arc::new(Mutex::new(Some(Routes {
        pending: HashMap::new(),
        outputs: HashMap::new(),
    })));
    let (closed_tx, closed_rx) = oneshot::channel();
    tokio::spawn(read_loop(
        BufReader::new(reader),
        Arc::clone(&shared),
        event_tx.clone(),
        Arc::clone(wake),
        closed_tx,
    ));
    let (writes, mut queued) = tmpsc::channel::<Vec<u8>>(WRITE_QUEUE);
    tokio::spawn(async move {
        while let Some(bytes) = queued.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    Ok(Connection {
        shared,
        writes,
        closed: closed_rx,
        version: daemon_version,
        next_id: AtomicU64::new(1),
    })
}

/// The events and the output frames arrive on the reader task; both are
/// routed through the shared table.
async fn read_loop(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    shared: Shared,
    event_tx: mpsc::Sender<UiEvent>,
    wake: Wake,
    closed: oneshot::Sender<()>,
) {
    while let Ok(frame) = read_frame(&mut reader).await {
        match frame.kind {
            FrameKind::Control => route_control(&shared, &frame.payload, &event_tx, &wake),
            FrameKind::TerminalOutput => route_output(&shared, &frame.payload, &event_tx, &wake),
        }
    }
    // The connection is over: failing the pending requests wakes every
    // waiter instead of leaving it on the timeout.
    teardown(&shared);
    let _ = closed.send(());
}

/// Route a control frame: a response completes its request, an event goes
/// to the UI.
fn route_control(shared: &Shared, payload: &[u8], event_tx: &mpsc::Sender<UiEvent>, wake: &Wake) {
    let Ok(incoming) = serde_json::from_slice::<Incoming>(payload) else {
        return;
    };
    match incoming {
        Incoming::Response(response) => {
            if let Some(resolver) = remove_pending(shared, response.id) {
                resolve(&resolver, Ok(response));
            }
        }
        Incoming::Event(event) => {
            push_event(event_tx, wake, UiEvent::Event(event));
        }
    }
}

/// Route a kind-1 frame to the subscriber that asked for that session.
fn route_output(shared: &Shared, payload: &[u8], event_tx: &mpsc::Sender<UiEvent>, wake: &Wake) {
    let Ok(output) = frame::decode_output(payload) else {
        return;
    };
    let session = SessionId::from_bytes(output.session_id);
    let chunk = Output {
        seq: output.seq,
        data: output.data.to_vec(),
    };
    let lagged = {
        let mut guard = shared.lock().unwrap();
        let Some(routes) = guard.as_mut() else {
            return;
        };
        let outcome = match routes.outputs.get(&session) {
            Some(subscriber) => subscriber.try_send(chunk),
            None => return,
        };
        match outcome {
            Ok(()) => false,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                // The consumer fell behind: end the subscription instead of
                // stalling every other frame. The UI resubscribes.
                routes.outputs.remove(&session);
                true
            }
        }
    };
    if lagged {
        push_event(
            event_tx,
            wake,
            UiEvent::Event(Event::TerminalLagged {
                session_id: session,
            }),
        );
    }
}

/// Fail every pending request: the connection is gone.
fn teardown(shared: &Shared) {
    let taken = shared.lock().unwrap().take();
    let Some(routes) = taken else {
        return;
    };
    for (_, resolver) in routes.pending {
        resolve(&resolver, Err(disconnected()));
    }
}

/// Push an event to the UI and wake its event loop.
fn push_event(event_tx: &mpsc::Sender<UiEvent>, wake: &Wake, event: UiEvent) {
    let _ = event_tx.send(event);
    wake();
}

async fn handshake(stream: &mut UnixStream, token: String) -> Result<String, BridgeError> {
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    };
    stream
        .write_all(&encode_control(&hello)?)
        .await
        .map_err(|_| disconnected())?;
    let reply = match read_frame(stream).await {
        Ok(Frame {
            kind: FrameKind::Control,
            payload,
        }) => payload,
        _ => return Err(disconnected()),
    };
    if let Ok(hello) = serde_json::from_slice::<HelloOk>(&reply) {
        return Ok(hello.daemon_version);
    }
    match serde_json::from_slice::<Response>(&reply) {
        Ok(Response {
            outcome: ResponseOutcome::Error { error },
            ..
        }) => Err(error.into()),
        _ => Err(BridgeError::new(
            Code::Daemon(ErrorCode::Malformed),
            "the daemon's handshake reply was not understood",
        )),
    }
}

/// A control frame's payload: either a response or an event.
#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Response(Response),
    Event(Event),
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Frame> {
    let mut header = [0_u8; 5];
    reader.read_exact(&mut header).await?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Err(io::Error::other("the daemon sent an oversized frame"));
    }
    let kind = FrameKind::try_from(header[4]).map_err(io::Error::other)?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(Frame { kind, payload })
}

fn encode_control(message: &impl Serialize) -> Result<Vec<u8>, BridgeError> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| BridgeError::new(Code::Daemon(ErrorCode::Internal), error.to_string()))?;
    frame::encode(&Frame {
        kind: FrameKind::Control,
        payload,
    })
    .map_err(|error| BridgeError::new(Code::Daemon(ErrorCode::FrameTooLarge), error.to_string()))
}

/// `GOKO_DAEMON_BIN`, else `gokod` beside the running executable.
fn daemon_bin() -> Result<PathBuf, BridgeError> {
    let exe = std::env::current_exe().map_err(|error| {
        BridgeError::new(
            Code::DaemonStartFailed,
            format!("cannot locate the running executable: {error}"),
        )
    })?;
    Ok(daemon_bin_from(std::env::var_os("GOKO_DAEMON_BIN"), &exe))
}

fn daemon_bin_from(var: Option<OsString>, exe: &Path) -> PathBuf {
    match var.filter(|var| !var.is_empty()) {
        Some(bin) => bin.into(),
        None => exe.with_file_name("gokod"),
    }
}

/// Start `gokod` in its own process group so it outlives the app, logging
/// to `daemon.log`. The daemon is not waited for: the supervisor
/// reconnects with backoff.
fn spawn_daemon(home: &Path, bin: &Path) -> Result<(), BridgeError> {
    let log_path = home.join("daemon.log");
    let failed = |what: &str, error: io::Error| {
        BridgeError::new(Code::DaemonStartFailed, format!("{what}: {error}"))
            .with_hint(format!("see {}", log_path.display()))
    };
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home)
        .map_err(|error| failed("cannot create the daemon directory", error))?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|error| failed("cannot open daemon.log", error))?;
    let mut command = Command::new(bin);
    command
        .env("GOKO_HOME", home)
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map_err(|error| failed("cannot open daemon.log", error))?,
        )
        .stderr(log);
    // Its own process group: the daemon survives the app's terminal.
    command.process_group(0);
    command
        .spawn()
        .map(drop)
        .map_err(|error| failed(&format!("cannot start {}", bin.display()), error))
}

/// `true` when the daemon version is at least `want`.
fn version_at_least(version: &str, want: (u64, u64, u64)) -> bool {
    let mut components = version.split('.');
    let (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) = (
        components.next().map(str::parse::<u64>),
        components.next().map(str::parse::<u64>),
        components.next().map(str::parse::<u64>),
        components.next(),
    ) else {
        return false;
    };
    (major, minor, patch) >= want
}

fn unavailable(error: io::Error) -> BridgeError {
    BridgeError::new(
        Code::DaemonUnavailable,
        format!("cannot reach gokod: {error}"),
    )
}

fn disconnected() -> BridgeError {
    BridgeError::new(Code::Disconnected, "the connection to gokod was lost")
}

fn timed_out() -> BridgeError {
    BridgeError::new(Code::Timeout, "gokod did not answer in time")
}

#[cfg(test)]
mod tests;

//! One authenticated connection to `gokod`, and the logic to start the daemon when absent.
//!
//! A reader task demultiplexes incoming frames: responses go to the request waiting on
//! their `id`, events to a broadcast, and terminal output to the subscriber registered
//! for the session. A writer task serializes outgoing frames, so a request that is
//! cancelled or times out can never leave half a frame on the wire.

use crate::error::{BridgeError, Code};
use protocol::frame::{self, Frame, FrameKind, MAX_FRAME_PAYLOAD};
use protocol::message::{ErrorCode, Event, Hello, HelloOk, Request, Response, ResponseOutcome};
use protocol::session::SessionId;
use protocol::PROTOCOL_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{sleep, timeout};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Pauses before each reconnect attempt after spawning the daemon.
const SPAWN_BACKOFF_MS: [u64; 5] = [50, 100, 200, 400, 800];
const EVENT_QUEUE: usize = 256;
const OUTPUT_QUEUE: usize = 256;
const WRITE_QUEUE: usize = 64;

/// Only one caller at a time may decide to start the daemon.
static SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One chunk of terminal output; `seq` is the stream offset of `data[0]`.
#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub seq: u64,
    pub data: Vec<u8>,
}

struct Routes {
    pending: HashMap<u64, oneshot::Sender<Response>>,
    outputs: HashMap<SessionId, mpsc::Sender<Output>>,
    events: broadcast::Sender<Event>,
}

/// `None` once the connection is gone. Dropping the senders is what wakes every waiter.
type Shared = Arc<Mutex<Option<Routes>>>;

pub struct DaemonClient {
    shared: Shared,
    writes: mpsc::Sender<Vec<u8>>,
    next_id: AtomicU64,
    daemon_version: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Response(Response),
    Event(Event),
}

impl DaemonClient {
    /// Connect to the daemon in `home` and authenticate.
    pub async fn connect(home: &Path) -> Result<Self, BridgeError> {
        let mut stream = UnixStream::connect(home.join("daemon.sock"))
            .await
            .map_err(unavailable)?;
        // Read the token only once the socket accepted: a live daemon writes it before
        // binding, so a stale file from a crashed daemon is never sent.
        let token = fs::read_to_string(home.join("daemon.token")).map_err(unavailable)?;
        let daemon_version = timeout(REQUEST_TIMEOUT, handshake(&mut stream, token))
            .await
            .map_err(|_| timed_out())??;

        let (reader, mut writer) = stream.into_split();
        let (events, _) = broadcast::channel(EVENT_QUEUE);
        let shared: Shared = Arc::new(Mutex::new(Some(Routes {
            pending: HashMap::new(),
            outputs: HashMap::new(),
            events,
        })));
        tokio::spawn(read_loop(BufReader::new(reader), Arc::clone(&shared)));

        let (writes, mut queued) = mpsc::channel::<Vec<u8>>(WRITE_QUEUE);
        tokio::spawn(async move {
            while let Some(bytes) = queued.recv().await {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            shared,
            writes,
            next_id: AtomicU64::new(1),
            daemon_version,
        })
    }

    /// Connect, starting `gokod` first when nobody answers. See `daemon_bin` for how the
    /// binary is found.
    pub async fn connect_or_spawn(home: &Path) -> Result<Self, BridgeError> {
        Self::connect_or_spawn_with(home, &daemon_bin()?).await
    }

    async fn connect_or_spawn_with(home: &Path, bin: &Path) -> Result<Self, BridgeError> {
        if let Some(done) = Self::try_connect(home).await {
            return done;
        }
        let _only_spawner = SPAWN_LOCK.lock().await;
        // The caller ahead of us may have started the daemon while we waited.
        if let Some(done) = Self::try_connect(home).await {
            return done;
        }
        spawn_daemon(home, bin)?;
        for pause in SPAWN_BACKOFF_MS {
            sleep(Duration::from_millis(pause)).await;
            if let Some(done) = Self::try_connect(home).await {
                return done;
            }
        }
        let log = home.join("daemon.log");
        Err(BridgeError::new(
            Code::DaemonStartFailed,
            "gokod was started but did not accept connections in time",
        )
        .with_hint(format!("see {}", log.display())))
    }

    /// `None` when the daemon is simply not there (yet), so trying again may help. A wrong
    /// token, an unsupported version, or a daemon that stopped answering will not heal by
    /// spawning another process.
    async fn try_connect(home: &Path) -> Option<Result<Self, BridgeError>> {
        match Self::connect(home).await {
            Err(error) if matches!(error.code, Code::DaemonUnavailable | Code::Disconnected) => {
                None
            }
            done => Some(done),
        }
    }

    /// Send the request built from a fresh id and return the daemon's `result`; a daemon
    /// `error` becomes a `BridgeError` with the same code.
    pub async fn request(&self, build: impl FnOnce(u64) -> Request) -> Result<Value, BridgeError> {
        self.request_within(REQUEST_TIMEOUT, build).await
    }

    pub async fn request_within(
        &self,
        limit: Duration,
        build: impl FnOnce(u64) -> Request,
    ) -> Result<Value, BridgeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let bytes = encode(&build(id))?;
        let (reply, answer) = oneshot::channel();
        self.routes(|routes| routes.pending.insert(id, reply))?;
        let exchange = async {
            self.writes.send(bytes).await.map_err(|_| disconnected())?;
            answer.await.map_err(|_| disconnected())
        };
        let outcome = timeout(limit, exchange).await;
        // After a timeout the reply must not linger in the table.
        let _ = self.routes(|routes| routes.pending.remove(&id));
        match outcome.map_err(|_| timed_out())??.outcome {
            ResponseOutcome::Result { result } => Ok(result),
            ResponseOutcome::Error { error } => Err(error.into()),
        }
    }

    /// Daemon events. The receiver reports `Closed` once the connection is lost.
    pub fn events(&self) -> Result<broadcast::Receiver<Event>, BridgeError> {
        self.routes(|routes| routes.events.subscribe())
    }

    /// Route kind-1 frames for `session` to the returned receiver, replacing any earlier
    /// subscriber. Register before sending `terminal.subscribe` so no chunk is missed.
    /// The receiver ends when the connection drops, or when it falls `OUTPUT_QUEUE`
    /// chunks behind: the reader never waits for a consumer, so the consumer must
    /// re-subscribe to recover.
    pub fn subscribe_output(
        &self,
        session: SessionId,
    ) -> Result<mpsc::Receiver<Output>, BridgeError> {
        self.routes(|routes| {
            let (sender, receiver) = mpsc::channel(OUTPUT_QUEUE);
            routes.outputs.insert(session, sender);
            receiver
        })
    }

    /// True once the connection is gone for good; a new `DaemonClient` is needed then.
    pub fn is_closed(&self) -> bool {
        self.shared.lock().unwrap().is_none()
    }

    pub fn daemon_version(&self) -> &str {
        &self.daemon_version
    }

    pub fn supports_live_cwd(&self) -> bool {
        supports_live_cwd(&self.daemon_version)
    }

    pub fn supports_flow_control(&self) -> bool {
        supports_flow_control(&self.daemon_version)
    }

    fn routes<T>(&self, use_routes: impl FnOnce(&mut Routes) -> T) -> Result<T, BridgeError> {
        let mut shared = self.shared.lock().unwrap();
        shared.as_mut().map(use_routes).ok_or_else(disconnected)
    }
}

/// `GOKO_HOME`, else `~/Library/Application Support/Goko` (the same rule as the daemon).
/// Shared with native frontends through `app_core::home`.
pub use app_core::home::home_dir;

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

/// Start `gokod` in its own session so it outlives the app, logging to `daemon.log`.
/// The daemon is not waited for: callers reconnect with backoff.
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
    // SAFETY: `setsid` is async-signal-safe and is the only call between fork and exec.
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    command
        .spawn()
        .map(drop)
        .map_err(|error| failed(&format!("cannot start {}", bin.display()), error))
}

async fn handshake(stream: &mut UnixStream, token: String) -> Result<String, BridgeError> {
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    };
    stream
        .write_all(&encode(&hello)?)
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

fn supports_live_cwd(version: &str) -> bool {
    version_at_least(version, (0, 1, 1))
}

fn supports_flow_control(version: &str) -> bool {
    version_at_least(version, (0, 1, 2))
}

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

async fn read_loop(mut reader: BufReader<tokio::net::unix::OwnedReadHalf>, shared: Shared) {
    while let Ok(frame) = read_frame(&mut reader).await {
        match frame.kind {
            FrameKind::Control => route_control(&shared, &frame.payload),
            FrameKind::TerminalOutput => route_output(&shared, &frame.payload),
        }
    }
    // Connection over: dropping the routes fails pending requests with `disconnected`
    // and ends every events and output receiver.
    shared.lock().unwrap().take();
}

fn route_control(shared: &Shared, payload: &[u8]) {
    let Ok(incoming) = serde_json::from_slice::<Incoming>(payload) else {
        return;
    };
    let mut shared = shared.lock().unwrap();
    let Some(routes) = shared.as_mut() else {
        return;
    };
    match incoming {
        Incoming::Response(response) => {
            if let Some(waiting) = routes.pending.remove(&response.id) {
                let _ = waiting.send(response);
            }
        }
        Incoming::Event(event) => {
            // No receivers is fine: nobody is listening yet.
            let _ = routes.events.send(event);
        }
    }
}

fn route_output(shared: &Shared, payload: &[u8]) {
    let Ok(output) = frame::decode_output(payload) else {
        return;
    };
    let session = SessionId::from_bytes(output.session_id);
    let mut shared = shared.lock().unwrap();
    let Some(routes) = shared.as_mut() else {
        return;
    };
    let Some(subscriber) = routes.outputs.get(&session) else {
        return;
    };
    let chunk = Output {
        seq: output.seq,
        data: output.data.to_vec(),
    };
    // Full or closed: end the subscription instead of stalling every other response.
    if subscriber.try_send(chunk).is_err() {
        routes.outputs.remove(&session);
    }
}

pub(crate) async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Frame> {
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

fn encode(message: &impl Serialize) -> Result<Vec<u8>, BridgeError> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| BridgeError::new(Code::Daemon(ErrorCode::Internal), error.to_string()))?;
    frame::encode(&Frame {
        kind: FrameKind::Control,
        payload,
    })
    .map_err(|error| BridgeError::new(Code::Daemon(ErrorCode::FrameTooLarge), error.to_string()))
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
mod connect;
#[cfg(test)]
mod spawn;

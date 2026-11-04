#![allow(dead_code)]

use daemon::paths::Paths;
use daemon::session::SessionManager;
use daemon::{auth, server};
use protocol::PROTOCOL_VERSION;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

#[path = "../../src/test_support.rs"]
mod temp_dir;
pub use temp_dir::TempDir;

/// A daemon server running in this process, stopped when dropped.
pub struct TestServer {
    pub paths: Paths,
    pub token: String,
    pub sessions: Arc<SessionManager>,
    _shutdown: oneshot::Sender<()>,
    _home: TempDir,
}

impl Drop for TestServer {
    /// Leave no session process behind, even when a test fails.
    fn drop(&mut self) {
        for info in self.sessions.list() {
            let _ = self.sessions.stop(info.id, true);
        }
    }
}

impl TestServer {
    pub fn start() -> Self {
        Self::start_in(TempDir::new("server"))
    }

    /// Serve out of `home`, which may already hold files such as a stale socket.
    pub fn start_in(home: TempDir) -> Self {
        let paths = Paths::at(home.path()).expect("create daemon directory");
        let token = auth::create_token(&paths).expect("create token");
        let listener = server::bind(&paths).expect("bind daemon socket");
        let token_text = fs::read_to_string(paths.token()).expect("read token file");
        let (shutdown, stop) = oneshot::channel::<()>();
        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
        let sessions = Arc::new(SessionManager::new());
        let worktree_operations_lock = paths.worktree_operations_lock();
        let shutdown_sessions = Arc::clone(&sessions);
        tokio::spawn(async move {
            let _ = server::serve(
                listener,
                token,
                Arc::clone(&shutdown_sessions),
                worktree_operations_lock,
                shutdown_tx,
                async {
                    tokio::select! {
                        _ = stop => {}
                        _ = shutdown_rx.recv() => {}
                    }
                },
            )
            .await;
            shutdown_sessions.shutdown().await;
        });
        Self {
            paths,
            token: token_text,
            sessions,
            _shutdown: shutdown,
            _home: home,
        }
    }

    pub async fn connect(&self) -> Client {
        Client::connect(&self.paths.socket()).await
    }

    pub async fn authenticated(&self) -> Client {
        Client::authenticated(&self.paths.socket(), &self.token).await
    }
}

/// A frame received from the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Control(Value),
    Output {
        session_id: [u8; 16],
        seq: u64,
        bytes: Vec<u8>,
    },
}

/// A test client speaking the framed protocol over the daemon socket. A reader task
/// parses frames into a channel, so waiting with a timeout never loses part of a frame.
pub struct Client {
    writer: OwnedWriteHalf,
    incoming: mpsc::UnboundedReceiver<Incoming>,
    /// Frames read past while waiting for a response, in arrival order.
    pending: VecDeque<Incoming>,
    next_id: u64,
}

impl Client {
    pub async fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket)
            .await
            .expect("connect to the daemon socket");
        let (mut reader, writer) = stream.into_split();
        let (sender, incoming) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(frame) = read_incoming(&mut reader).await {
                if sender.send(frame).is_err() {
                    return;
                }
            }
        });
        Self {
            writer,
            incoming,
            pending: VecDeque::new(),
            next_id: 1,
        }
    }

    pub async fn authenticated(socket: &Path, token: &str) -> Self {
        let mut client = Self::connect(socket).await;
        client.hello(PROTOCOL_VERSION, token).await;
        let reply = client.recv().await.expect("handshake reply");
        assert!(
            reply.get("daemon_version").is_some(),
            "expected HelloOk, got {reply}"
        );
        client
    }

    pub async fn hello(&mut self, protocol_version: u32, token: &str) {
        self.send(&json!({"protocol_version": protocol_version, "token": token}))
            .await;
    }

    /// Send a JSON control frame.
    pub async fn send(&mut self, message: &Value) {
        let payload = serde_json::to_vec(message).unwrap();
        self.send_raw(&frame_bytes(0, &payload)).await;
    }

    pub async fn send_raw(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .await
            .expect("write to the daemon");
    }

    /// The next frame, or `None` once the daemon closed the connection.
    pub async fn next(&mut self) -> Option<Incoming> {
        if let Some(frame) = self.pending.pop_front() {
            return Some(frame);
        }
        timeout(Duration::from_secs(5), self.incoming.recv())
            .await
            .expect("timed out waiting for the daemon")
    }

    /// The next frame if one arrives within `wait`; `None` on silence or close.
    pub async fn next_within(&mut self, wait: Duration) -> Option<Incoming> {
        if let Some(frame) = self.pending.pop_front() {
            return Some(frame);
        }
        timeout(wait, self.incoming.recv()).await.ok().flatten()
    }

    /// The next control message, or `None` once the daemon closed the connection.
    pub async fn recv(&mut self) -> Option<Value> {
        match self.next().await? {
            Incoming::Control(value) => Some(value),
            other => panic!("expected a control message, got {other:?}"),
        }
    }

    /// Send a request and return its response. Events and output frames that arrive first
    /// are kept for later `next` calls. Requests without an `id` are sent as written.
    pub async fn request(&mut self, mut message: Value) -> Value {
        if message.get("id").is_none() {
            message["id"] = json!(self.next_id);
            self.next_id += 1;
        }
        let id = message["id"].clone();
        self.send(&message).await;

        let mut skipped = Vec::new();
        let response = loop {
            let frame = timeout(Duration::from_secs(5), self.incoming.recv())
                .await
                .expect("timed out waiting for the response")
                .expect("the daemon closed the connection before responding");
            match &frame {
                Incoming::Control(value) if value.get("id") == Some(&id) => break value.clone(),
                _ => skipped.push(frame),
            }
        };
        self.pending.extend(skipped);
        response
    }
}

pub async fn read_incoming(reader: &mut (impl AsyncReadExt + Unpin)) -> Option<Incoming> {
    let mut header = [0_u8; 5];
    reader.read_exact(&mut header).await.ok()?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await.ok()?;
    match header[4] {
        0 => Some(Incoming::Control(
            serde_json::from_slice(&payload).expect("the daemon sends JSON"),
        )),
        1 => {
            assert!(payload.len() >= 24, "output frame shorter than its header");
            Some(Incoming::Output {
                session_id: payload[..16].try_into().unwrap(),
                seq: u64::from_be_bytes(payload[16..24].try_into().unwrap()),
                bytes: payload[24..].to_vec(),
            })
        }
        kind => panic!("the daemon sent an unknown frame kind {kind}"),
    }
}

pub fn frame_bytes(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.push(kind);
    bytes.extend_from_slice(payload);
    bytes
}

pub const GOKOD: &str = env!("CARGO_BIN_EXE_gokod");

/// A running `gokod` that is killed when dropped, so a failed test leaves nothing behind.
pub struct Daemon(pub Child);

impl Daemon {
    pub fn start(home: &TempDir) -> Self {
        let child = Command::new(GOKOD)
            .env("GOKO_HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start gokod");
        Self(child)
    }

    pub fn pid(&self) -> libc::pid_t {
        self.0.id() as libc::pid_t
    }

    /// Wait for the process to exit, returning whether it did within `limit`.
    pub async fn exited_within(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(status) = self.0.try_wait().expect("poll gokod") {
                return Some(status);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub async fn connect(paths: &Paths) -> Client {
    // A successful connect proves a live daemon bound the socket, and the daemon writes
    // the token before binding, so the file read below is current even after a crash.
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(paths.socket()).await.is_err() {
        assert!(Instant::now() < deadline, "gokod did not start serving");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let token = std::fs::read_to_string(paths.token()).expect("read the token file");
    Client::authenticated(&paths.socket(), &token).await
}

pub fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

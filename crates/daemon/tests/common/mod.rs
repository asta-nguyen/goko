#![allow(dead_code)]

use daemon::paths::Paths;
use daemon::{auth, server};
use protocol::PROTOCOL_VERSION;
use serde_json::{json, Value};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::time::timeout;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("gokod-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A daemon server running in this process, stopped when dropped.
pub struct TestServer {
    pub paths: Paths,
    pub token: String,
    _shutdown: oneshot::Sender<()>,
    _home: TempDir,
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
        tokio::spawn(server::serve(listener, token, async {
            let _ = stop.await;
        }));
        Self {
            paths,
            token: token_text,
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

/// A test client speaking the framed protocol over the daemon socket.
pub struct Client(UnixStream);

impl Client {
    pub async fn connect(socket: &Path) -> Self {
        Self(
            UnixStream::connect(socket)
                .await
                .expect("connect to the daemon socket"),
        )
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
        self.0.write_all(bytes).await.expect("write to the daemon");
    }

    /// The next control message, or `None` once the daemon closed the connection.
    pub async fn recv(&mut self) -> Option<Value> {
        timeout(Duration::from_secs(5), async {
            let mut header = [0_u8; 5];
            match self.0.read_exact(&mut header).await {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                    ) =>
                {
                    return None;
                }
                Err(error) => panic!("read from the daemon: {error}"),
            }
            assert_eq!(header[4], 0, "the daemon sends control frames");
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            let mut payload = vec![0_u8; length as usize];
            self.0
                .read_exact(&mut payload)
                .await
                .expect("read the frame payload");
            Some(serde_json::from_slice(&payload).expect("the daemon sends JSON"))
        })
        .await
        .expect("timed out waiting for the daemon")
    }

    /// Send a request and return the daemon's reply.
    pub async fn request(&mut self, message: Value) -> Value {
        self.send(&message).await;
        self.recv().await.expect("a reply to the request")
    }
}

pub fn frame_bytes(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.push(kind);
    bytes.extend_from_slice(payload);
    bytes
}

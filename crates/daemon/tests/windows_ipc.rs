//! Windows integration tests: named-pipe transport, ConPTY sessions, and the
//! Job-Object stop contract. The Unix suites in this folder are `cfg(unix)`;
//! these run on the `windows-latest` CI job.
#![cfg(windows)]

use daemon::paths::Paths;
use daemon::session::SessionManager;
use daemon::{auth, server};
use protocol::frame::{self, Frame, FrameKind};
use protocol::PROTOCOL_VERSION;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temp `GOKO_HOME` removed on drop.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "goko-win-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create the temp home");
        Self(dir)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running in-process daemon and one authenticated pipe client.
struct Harness {
    home: Home,
    token: String,
    serve: tokio::task::JoinHandle<()>,
    client: tokio::net::windows::named_pipe::NamedPipeClient,
    buffer: Vec<u8>,
    next_id: u64,
}

impl Harness {
    async fn start(tag: &str) -> Self {
        let home = Home::new(tag);
        let paths = Paths::at(&home.0).expect("paths");
        let token = auth::create_token(&paths).expect("token");
        let token_text = std::fs::read_to_string(paths.token()).expect("token file");
        let listener = server::bind(&paths).expect("bind the pipe");
        let sessions = Arc::new(SessionManager::new());
        let (shutdown_tx, _shutdown_rx) = tokio::sync::mpsc::unbounded_channel();
        let operations_lock = paths.worktree_operations_lock();
        let serve = tokio::spawn(async move {
            let _ = server::serve(
                listener,
                token,
                sessions,
                operations_lock,
                shutdown_tx,
                std::future::pending(),
            )
            .await;
        });
        let name = daemon::net::pipe_name(paths.dir());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("could not connect to {name}: {error}"),
            }
        };
        let mut harness = Self {
            home,
            token: token_text,
            serve,
            client,
            buffer: Vec::new(),
            next_id: 0,
        };
        harness.hello().await;
        harness
    }

    async fn hello(&mut self) {
        let payload = json!({
            "token": self.token.trim(),
            "protocol_version": PROTOCOL_VERSION,
        });
        self.write_control(&payload).await;
        let reply = self.read_control().await;
        assert_eq!(
            reply["daemon_version"],
            json!(env!("CARGO_PKG_VERSION")),
            "hello reply: {reply}"
        );
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut payload = params;
        payload["type"] = json!(method);
        payload["id"] = json!(id);
        self.write_control(&payload).await;
        loop {
            let reply = self.read_control().await;
            if reply["id"] == json!(id) {
                return reply;
            }
        }
    }

    async fn write_control(&mut self, payload: &Value) {
        let encoded = frame::encode(&Frame {
            kind: FrameKind::Control,
            payload: serde_json::to_vec(payload).unwrap(),
        })
        .unwrap();
        self.client.write_all(&encoded).await.unwrap();
    }

    async fn read_control(&mut self) -> Value {
        loop {
            if let Ok((Frame { payload, .. }, used)) = frame::decode(&self.buffer) {
                self.buffer.drain(..used);
                if let Ok(value) = serde_json::from_slice::<Value>(&payload) {
                    return value;
                }
                continue;
            }
            let mut chunk = [0_u8; 8192];
            let count = self.client.read(&mut chunk).await.expect("read a frame");
            assert!(count > 0, "the daemon closed the pipe");
            self.buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.serve.abort();
        let _ = &self.home;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pipe_round_trip_lists_sessions() {
    let mut harness = Harness::start("roundtrip").await;
    let reply = harness.request("session.list", json!({})).await;
    assert_eq!(reply["result"]["sessions"], json!([]), "reply: {reply}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_bind_on_the_same_home_is_already_running() {
    let home = Home::new("busy");
    let paths = Paths::at(&home.0).expect("paths");
    let _listener = server::bind(&paths).expect("first bind");
    match server::bind(&paths) {
        Ok(_) => panic!("the second bind must fail while the first listener lives"),
        Err(error) => assert!(
            // ERROR_ACCESS_DENIED is what FILE_FLAG_FIRST_PIPE_INSTANCE reports
            // when the name already exists; ERROR_PIPE_BUSY (231) when all are busy.
            error.kind() == std::io::ErrorKind::AddrInUse
                || matches!(error.raw_os_error(), Some(5 | 231)),
            "unexpected error: {error:?}"
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn conpty_session_spawns_and_stop_kills_the_tree() {
    let mut harness = Harness::start("conpty").await;
    let cwd = harness.home.0.clone();
    let reply = harness
        .request(
            "session.create",
            json!({
                "argv": ["cmd.exe", "/c", "timeout", "/t", "60"],
                "cwd": cwd,
                "cols": 80,
                "rows": 24,
                "title": "windows-ipc",
            }),
        )
        .await;
    let id = reply["result"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("create reply: {reply}"))
        .to_owned();

    let stop = harness
        .request("session.stop", json!({ "session_id": id, "force": false }))
        .await;
    assert!(stop.get("error").is_none(), "stop reply: {stop}");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let get = harness
            .request("session.get", json!({ "session_id": id }))
            .await;
        let state = get["result"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if state == "exited" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "session did not exit after stop: {get}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

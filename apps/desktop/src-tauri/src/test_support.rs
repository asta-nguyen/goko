//! Shared test fixtures: a temporary `GOKO_HOME` and the built `gokod`.

use crate::commands::CreateArgs;
use crate::daemon_client::read_frame;
use protocol::frame::{self, Frame, FrameKind};
use protocol::session::SessionId;
use serde_json::{json, Value};
use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory removed on drop. Keep labels short: a unix socket path
/// must fit in about 100 bytes.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("goko-br-{label}-{}-{id}", std::process::id()));
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

/// A temporary `GOKO_HOME` (not created until something uses it). Dropping it kills every
/// daemon that was started with it, identified by the pids `gokod()` records in
/// `<home>/spawns`, so a failing test leaves no process behind.
pub struct Home {
    path: PathBuf,
    _dir: TempDir,
}

impl Home {
    pub fn new(label: &str) -> Self {
        // Also points `GOKO_DAEMON_BIN` at the counting `gokod`, which everything that
        // starts a daemon through `connect_or_spawn` needs.
        gokod();
        let dir = TempDir::new(label);
        Self {
            path: dir.0.join("h"),
            _dir: dir,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `gokod` against this home and wait until it accepts connections.
    pub fn start_daemon(&self) {
        let mut child = Command::new(gokod())
            .env("GOKO_HOME", &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start gokod");
        // Reap it whenever it ends, so a killed daemon does not linger as a zombie.
        std::thread::spawn(move || child.wait());
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::os::unix::net::UnixStream::connect(self.path.join("daemon.sock")).is_err() {
            assert!(Instant::now() < deadline, "gokod did not start serving");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Pids of every `gokod` launched with this home, oldest first. One per launch, so
    /// the length is the number of times a daemon was started.
    pub fn spawns(&self) -> Vec<libc::pid_t> {
        fs::read_to_string(self.path.join("spawns"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect()
    }

    pub fn signal_daemons(&self, signal: libc::c_int) {
        for pid in self.spawns() {
            // SAFETY: plain kill(2) on a pid this test started.
            unsafe { libc::kill(pid, signal) };
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.signal_daemons(libc::SIGKILL);
    }
}

/// Path of an executable that records its pid in `$GOKO_HOME/spawns` and then becomes the
/// real `gokod`. Builds `gokod` first, once per test process.
pub fn gokod() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "daemon", "--bin", "gokod"])
            .status()
            .expect("run cargo");
        assert!(status.success(), "cargo build -p daemon --bin gokod failed");

        // target/debug/deps/<test binary> -> target/debug/gokod
        let real = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("gokod");
        assert!(real.exists(), "{} was not built", real.display());
        // Same content on every run, replaced atomically, and written before any test
        // spawns a process: no forked child can hold it open for writing while it runs.
        let script = real.with_file_name("gokod-spawn-log");
        let body = format!(
            "#!/bin/sh\nmkdir -p \"$GOKO_HOME\" && echo $$ >> \"$GOKO_HOME/spawns\"\nexec '{}' \"$@\"\n",
            real.display()
        );
        if fs::read_to_string(&script).ok().as_deref() != Some(body.as_str()) {
            let staged = real.with_file_name(format!("gokod-spawn-log.{}", std::process::id()));
            fs::write(&staged, &body).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
            fs::rename(&staged, &script).unwrap();
        }
        // Set once, before any test spawns a process (`Home::new` calls this first).
        std::env::set_var("GOKO_DAEMON_BIN", &script);
        script
    })
}

/// `CreateArgs` for a free-form `command` launched in `cwd` at 80x24.
pub fn create_args(command: &str, cwd: &str) -> CreateArgs {
    CreateArgs {
        command: Some(command.into()),
        agent: None,
        project_id: None,
        cwd: Some(cwd.into()),
        title: None,
        cols: 80,
        rows: 24,
    }
}

/// Stand-in daemon for what the real one cannot be made to do on demand (lag notices,
/// dropped connections). It accepts any number of connections, completes each handshake,
/// and runs `serve(connection_index, stream)` on it.
pub fn fake_daemon<F, Fut>(home: &Home, serve: F) -> tokio::task::JoinHandle<()>
where
    F: Fn(usize, UnixStream) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fs::create_dir_all(home.path()).unwrap();
    fs::write(home.path().join("daemon.token"), "tok").unwrap();
    let listener = UnixListener::bind(home.path().join("daemon.sock")).unwrap();
    tokio::spawn(async move {
        for index in 0.. {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_json(&mut stream).await["token"], "tok");
            send_json(&mut stream, json!({"daemon_version": "fake"})).await;
            tokio::spawn(serve(index, stream));
        }
    })
}

/// The next control message, or `None` once the client is gone.
pub async fn read_request(stream: &mut UnixStream) -> Option<Value> {
    let frame = read_frame(stream).await.ok()?;
    assert_eq!(frame.kind, FrameKind::Control);
    Some(serde_json::from_slice(&frame.payload).unwrap())
}

pub async fn read_json(stream: &mut UnixStream) -> Value {
    read_request(stream)
        .await
        .expect("the client sent a message")
}

pub async fn send_json(stream: &mut UnixStream, message: Value) {
    send(
        stream,
        FrameKind::Control,
        serde_json::to_vec(&message).unwrap(),
    )
    .await;
}

/// Answer `request` with `result`.
pub async fn reply(stream: &mut UnixStream, request: &Value, result: Value) {
    send_json(stream, json!({"id": request["id"], "result": result})).await;
}

pub async fn send_output(stream: &mut UnixStream, session: SessionId, seq: u64, data: &[u8]) {
    let payload = frame::encode_output(*session.as_bytes(), seq, data).unwrap();
    send(stream, FrameKind::TerminalOutput, payload).await;
}

async fn send(stream: &mut UnixStream, kind: FrameKind, payload: Vec<u8>) {
    let bytes = frame::encode(&Frame { kind, payload }).unwrap();
    stream.write_all(&bytes).await.unwrap();
}

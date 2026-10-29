mod common;

use common::{Client, TempDir};
use daemon::paths::Paths;
use serde_json::json;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

const GOKOD: &str = env!("CARGO_BIN_EXE_gokod");

/// A running `gokod` that is killed when dropped, so a failed test leaves nothing behind.
struct Daemon(Child);

impl Daemon {
    fn start(home: &TempDir) -> Self {
        let child = Command::new(GOKOD)
            .env("GOKO_HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start gokod");
        Self(child)
    }

    fn pid(&self) -> libc::pid_t {
        self.0.id() as libc::pid_t
    }

    /// Wait for the process to exit, returning whether it did within `limit`.
    async fn exited_within(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
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

async fn connect(paths: &Paths) -> Client {
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

#[tokio::test]
async fn gokod_serves_health_and_a_second_instance_exits_cleanly() {
    let home = TempDir::new("lifecycle-single");
    let paths = Paths::at(home.path()).unwrap();
    let _daemon = Daemon::start(&home);
    let mut client = connect(&paths).await;

    let reply = client
        .request(json!({"type": "system.health", "id": 1}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");

    let second = Command::new(GOKOD)
        .env("GOKO_HOME", home.path())
        .output()
        .expect("run a second gokod");
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already running"));

    let reply = client
        .request(json!({"type": "system.health", "id": 2}))
        .await;
    assert_eq!(
        reply["result"]["status"], "ok",
        "the first daemon keeps serving"
    );
}

#[tokio::test]
async fn sigterm_removes_the_socket_and_stops_within_two_seconds() {
    let home = TempDir::new("lifecycle-sigterm");
    let paths = Paths::at(home.path()).unwrap();
    let mut daemon = Daemon::start(&home);
    connect(&paths).await;

    assert_eq!(unsafe { libc::kill(daemon.pid(), libc::SIGTERM) }, 0);

    let status = daemon
        .exited_within(Duration::from_secs(2))
        .await
        .expect("gokod should exit within 2 s of SIGTERM");
    assert!(status.success());
    assert!(
        !paths.socket().exists(),
        "the socket file should be removed"
    );
}

#[tokio::test]
async fn a_stale_socket_from_a_crashed_daemon_is_recovered() {
    let home = TempDir::new("lifecycle-stale");
    let paths = Paths::at(home.path()).unwrap();
    {
        let mut crashed = Daemon::start(&home);
        connect(&paths).await;
        // SIGKILL leaves daemon.sock behind, like a crash.
        crashed.0.kill().expect("kill gokod");
        crashed.0.wait().expect("reap gokod");
    }
    assert!(
        paths.socket().exists(),
        "SIGKILL should leave the socket file"
    );

    let _daemon = Daemon::start(&home);
    let mut client = connect(&paths).await;

    let reply = client
        .request(json!({"type": "system.health", "id": 1}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");
}

#[test]
fn version_flag_still_works_and_unknown_flags_are_a_usage_error() {
    let version = Command::new(GOKOD)
        .arg("--version")
        .output()
        .expect("run gokod --version");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        concat!("gokod ", env!("CARGO_PKG_VERSION"), "\n")
    );

    let bogus = Command::new(GOKOD)
        .arg("--bogus")
        .output()
        .expect("run gokod --bogus");
    assert_eq!(bogus.status.code(), Some(2));
}

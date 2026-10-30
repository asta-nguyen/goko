mod common;

use common::{connect, Daemon, TempDir, GOKOD};
use daemon::paths::Paths;
use serde_json::json;
use std::process::Command;
use std::time::Duration;

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

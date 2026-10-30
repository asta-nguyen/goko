use super::*;
use crate::test_support::{gokod, Home};
use std::time::Instant;
use tokio::task::JoinSet;

fn health(id: u64) -> Request {
    Request::SystemHealth { id }
}

#[tokio::test]
async fn absent_daemon_is_started_and_connected() {
    let home = Home::new("start");

    let client = DaemonClient::connect_or_spawn_with(home.path(), gokod())
        .await
        .unwrap();

    assert_eq!(client.request(health).await.unwrap()["status"], "ok");
    assert_eq!(home.spawns().len(), 1);
    assert!(home.path().join("daemon.log").exists());
}

#[tokio::test]
async fn a_second_call_reuses_the_running_daemon() {
    let home = Home::new("reuse");
    let _first = DaemonClient::connect_or_spawn_with(home.path(), gokod())
        .await
        .unwrap();

    let second = DaemonClient::connect_or_spawn_with(home.path(), gokod())
        .await
        .unwrap();

    assert_eq!(second.request(health).await.unwrap()["status"], "ok");
    assert_eq!(home.spawns().len(), 1);
}

#[tokio::test]
async fn eight_concurrent_calls_start_exactly_one_daemon() {
    let home = Home::new("race");
    let mut calls = JoinSet::new();
    for _ in 0..8 {
        let path = home.path().to_owned();
        calls.spawn(async move { DaemonClient::connect_or_spawn_with(&path, gokod()).await });
    }

    while let Some(done) = calls.join_next().await {
        let client = done.unwrap().unwrap();
        assert_eq!(client.request(health).await.unwrap()["status"], "ok");
    }
    assert_eq!(home.spawns().len(), 1);
}

#[tokio::test]
async fn bogus_daemon_bin_is_start_failed_with_the_log_path() {
    let home = Home::new("bogus");

    let result =
        DaemonClient::connect_or_spawn_with(home.path(), Path::new("/nonexistent/gokod")).await;

    let error = result.err().unwrap();
    assert_eq!(error.code, Code::DaemonStartFailed);
    assert!(
        error.message.contains("/nonexistent/gokod"),
        "{}",
        error.message
    );
    assert!(error.hint.unwrap().contains("daemon.log"));
}

#[tokio::test]
async fn a_daemon_that_never_serves_exhausts_the_backoff() {
    let home = Home::new("mute");
    let started = Instant::now();

    // `true` starts fine and exits at once, so nothing ever listens.
    let result = DaemonClient::connect_or_spawn_with(home.path(), Path::new("/usr/bin/true")).await;

    let error = result.err().unwrap();
    assert_eq!(error.code, Code::DaemonStartFailed);
    assert!(error.hint.unwrap().ends_with("daemon.log"));
    // 50 + 100 + 200 + 400 + 800 ms of pauses, and no more attempts than that.
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(1550) && took < Duration::from_secs(5),
        "{took:?}"
    );
}

#[tokio::test]
async fn wrong_token_is_not_retried_or_respawned() {
    let home = Home::new("auth");
    home.start_daemon();
    fs::write(home.path().join("daemon.token"), "0".repeat(64)).unwrap();

    let result = DaemonClient::connect_or_spawn_with(home.path(), gokod()).await;

    assert_eq!(
        result.err().unwrap().code,
        Code::Daemon(ErrorCode::AuthFailed)
    );
    assert_eq!(
        home.spawns().len(),
        1,
        "only the daemon started by the test"
    );
}

#[test]
fn daemon_bin_is_the_env_override_else_beside_the_executable() {
    let exe = Path::new("/Applications/Goko.app/Contents/MacOS/desktop");
    assert_eq!(
        daemon_bin_from(Some("/opt/gokod".into()), exe),
        PathBuf::from("/opt/gokod")
    );
    for var in [None, Some("".into())] {
        assert_eq!(
            daemon_bin_from(var, exe),
            PathBuf::from("/Applications/Goko.app/Contents/MacOS/gokod")
        );
    }
}

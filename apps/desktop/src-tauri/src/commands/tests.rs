use super::*;
use crate::test_support::Home;
use protocol::session::SessionState;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn bridge(home: &Home) -> Bridge {
    Bridge::new(home.path().to_owned())
}

async fn create(bridge: &Bridge, command: &str) -> SessionInfo {
    bridge
        .session_create(command, "/tmp".into(), 80, 24, None)
        .await
        .unwrap()
}

async fn wait_for_state(bridge: &Bridge, id: SessionId, wanted: SessionState) -> SessionInfo {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let sessions = bridge.session_list().await.unwrap();
        let session = sessions
            .into_iter()
            .find(|session| session.id == id)
            .unwrap();
        if session.state == wanted {
            return session;
        }
        assert!(Instant::now() < deadline, "session is {:?}", session.state);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_session_goes_from_create_through_write_and_resize_to_exited() {
    let home = Home::new("cmd-life");
    let bridge = bridge(&home);

    let session = bridge
        .session_create("cat", "/tmp".into(), 80, 24, Some("my cat".into()))
        .await
        .unwrap();
    assert_eq!(
        (session.title.as_str(), session.argv.clone()),
        ("my cat", vec!["cat".to_owned()])
    );

    let listed = bridge.session_list().await.unwrap();
    assert!(listed.iter().any(|listed| listed.id == session.id));

    bridge.terminal_write(session.id, "hi\n").await.unwrap();
    bridge.terminal_resize(session.id, 100, 30).await.unwrap();
    bridge.session_stop(session.id, false).await.unwrap();

    let exited = wait_for_state(&bridge, session.id, SessionState::Exited).await;
    assert!(exited.ended_at.is_some());
}

#[tokio::test]
async fn the_command_is_split_like_a_shell_would_without_running_one() {
    let home = Home::new("cmd-split");
    let bridge = bridge(&home);

    let session = create(&bridge, r#"sh -c 'exit 7' "two words""#).await;

    assert_eq!(session.argv, ["sh", "-c", "exit 7", "two words"]);
    let exited = wait_for_state(&bridge, session.id, SessionState::Exited).await;
    assert_eq!(exited.exit_code, Some(7));
}

#[tokio::test]
async fn an_unbalanced_quote_is_invalid_argument_and_never_reaches_the_daemon() {
    let home = Home::new("cmd-quote");
    let bridge = bridge(&home);

    let error = bridge
        .session_create("echo 'oops", "/tmp".into(), 80, 24, None)
        .await
        .unwrap_err();

    assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    assert!(
        home.spawns().is_empty(),
        "no daemon was needed to reject it"
    );
}

#[tokio::test]
async fn a_missing_directory_gives_a_failed_session_not_an_error() {
    let home = Home::new("cmd-cwd");
    let bridge = bridge(&home);

    let session = bridge
        .session_create("cat", "/nonexistent/dir".into(), 80, 24, None)
        .await
        .unwrap();

    assert_eq!(session.state, SessionState::Failed);
    assert!(session.error.is_some());
}

#[tokio::test]
async fn daemon_errors_keep_their_code() {
    let home = Home::new("cmd-err");
    let bridge = bridge(&home);
    let missing = SessionId::new();

    let write = bridge.terminal_write(missing, "x").await.unwrap_err();
    let stop = bridge.session_stop(missing, false).await.unwrap_err();

    assert_eq!(write.code, Code::Daemon(ErrorCode::NotFound));
    assert_eq!(stop.code, Code::Daemon(ErrorCode::NotFound));

    let session = create(&bridge, "true").await;
    wait_for_state(&bridge, session.id, SessionState::Exited).await;
    let write = bridge.terminal_write(session.id, "x").await.unwrap_err();
    assert_eq!(write.code, Code::Daemon(ErrorCode::InvalidArgument));
}

#[tokio::test]
async fn status_never_starts_the_daemon_and_follows_the_connection() {
    let home = Home::new("cmd-stat");
    let bridge = bridge(&home);

    let before = bridge.daemon_status();
    assert!(!before.connected);
    assert_eq!(before.home, home.path().display().to_string());
    assert!(!before.default_shell.is_empty());
    assert!(home.spawns().is_empty());

    bridge.session_list().await.unwrap();
    assert!(bridge.daemon_status().connected);
}

#[tokio::test]
async fn racing_first_uses_make_one_connection_and_start_one_daemon() {
    let home = Home::new("cmd-race");
    let bridge = bridge(&home);

    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let bridge = bridge.clone();
        calls.spawn(async move { bridge.client().await.unwrap() });
    }
    let mut clients = Vec::new();
    while let Some(client) = calls.join_next().await {
        clients.push(client.unwrap());
    }

    assert!(clients
        .iter()
        .all(|client| Arc::ptr_eq(client, &clients[0])));
    assert_eq!(home.spawns().len(), 1);
}

#[test]
fn the_default_shell_falls_back_to_zsh() {
    assert_eq!(default_shell(Some("/bin/fish".into())), "/bin/fish");
    for shell in [None, Some("".into())] {
        assert_eq!(default_shell(shell), "/bin/zsh");
    }
}

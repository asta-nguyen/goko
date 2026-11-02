use super::*;
use crate::store::Storage;
use crate::test_support::Home;
use protocol::session::SessionState;
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn bridge(home: &Home) -> Bridge {
    Bridge::new(home.path().to_owned())
}

fn storage(home: &Home) -> Storage {
    Storage::open(home.path())
}

fn args(command: &str, cwd: &str) -> CreateArgs {
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

fn blank() -> CreateArgs {
    CreateArgs {
        command: None,
        agent: None,
        project_id: None,
        cwd: None,
        title: None,
        cols: 80,
        rows: 24,
    }
}

async fn create(bridge: &Bridge, storage: &Storage, command: &str) -> SessionInfo {
    bridge
        .session_create(storage, args(command, "/tmp"))
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
    let storage = storage(&home);

    let session = bridge
        .session_create(
            &storage,
            CreateArgs {
                command: Some("cat".into()),
                title: Some("my cat".into()),
                ..args("cat", "/tmp")
            },
        )
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
    let storage = storage(&home);

    let session = create(&bridge, &storage, r#"sh -c 'exit 7' "two words""#).await;

    assert_eq!(session.argv, ["sh", "-c", "exit 7", "two words"]);
    let exited = wait_for_state(&bridge, session.id, SessionState::Exited).await;
    assert_eq!(exited.exit_code, Some(7));
}

#[tokio::test]
async fn an_unbalanced_quote_is_invalid_argument_and_never_reaches_the_daemon() {
    let home = Home::new("cmd-quote");
    let bridge = bridge(&home);
    let storage = storage(&home);

    let error = bridge
        .session_create(&storage, args("echo 'oops", "/tmp"))
        .await
        .unwrap_err();

    assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    assert!(
        home.spawns().is_empty(),
        "no daemon was needed to reject it"
    );
}

#[tokio::test]
async fn the_validation_matrix_is_rejected_before_the_daemon() {
    let home = Home::new("cmd-matrix");
    let bridge = bridge(&home);
    let storage = storage(&home);

    let cases = [
        (
            CreateArgs {
                command: Some("cat".into()),
                agent: Some("shell".into()),
                ..args("cat", "/tmp")
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        // Neither `command` nor `agent`.
        (
            CreateArgs {
                cwd: Some("/tmp".into()),
                ..blank()
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        (
            CreateArgs {
                project_id: Some("p".into()),
                ..args("cat", "/tmp")
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        // Neither `project_id` nor `cwd`.
        (
            CreateArgs {
                command: Some("cat".into()),
                ..blank()
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        (
            CreateArgs {
                agent: Some("bogus".into()),
                ..args("cat", "/tmp")
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        (
            CreateArgs {
                command: None,
                agent: Some("bogus".into()),
                ..args("cat", "/tmp")
            },
            Code::Daemon(ErrorCode::InvalidArgument),
        ),
        (
            CreateArgs {
                project_id: Some("unregistered".into()),
                cwd: None,
                ..args("cat", "/tmp")
            },
            Code::Daemon(ErrorCode::NotFound),
        ),
    ];
    for (case, wanted) in cases {
        let error = bridge.session_create(&storage, case).await.unwrap_err();
        assert_eq!(error.code, wanted);
    }
    assert!(home.spawns().is_empty(), "no daemon was needed to reject");

    // A project whose root vanished is invalid_argument, also before the daemon.
    let gone = home.path().join("gone");
    fs::create_dir_all(&gone).unwrap();
    let project = storage
        .get()
        .unwrap()
        .add_project("gone", &gone.display().to_string())
        .unwrap();
    fs::remove_dir(&gone).unwrap();
    let error = bridge
        .session_create(
            &storage,
            CreateArgs {
                project_id: Some(project.id),
                cwd: None,
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    assert!(home.spawns().is_empty());
}

#[tokio::test]
async fn a_missing_directory_gives_a_failed_session_not_an_error() {
    let home = Home::new("cmd-cwd");
    let bridge = bridge(&home);
    let storage = storage(&home);

    let session = bridge
        .session_create(&storage, args("cat", "/nonexistent/dir"))
        .await
        .unwrap();

    assert_eq!(session.state, SessionState::Failed);
    assert!(session.error.is_some());
}

#[tokio::test]
async fn a_shell_agent_session_records_metadata_and_last_agent() {
    let home = Home::new("cmd-agent");
    let bridge = bridge(&home);
    let storage = storage(&home);

    let session = bridge
        .session_create(
            &storage,
            CreateArgs {
                command: None,
                agent: Some("shell".into()),
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap();
    // The launched shell is real; stop it before the home goes away.
    bridge.session_stop(session.id, false).await.unwrap();

    let store = storage.get().unwrap();
    let metas = store.list_session_meta().unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].session_id, session.id.as_uuid().to_string());
    assert_eq!(metas[0].agent, "shell");
    assert_eq!(metas[0].project_id, None);
    assert_eq!(
        store.get_setting("last_agent").unwrap().as_deref(),
        Some("shell")
    );
}

#[tokio::test]
async fn a_project_session_runs_in_its_root_and_records_the_project() {
    let home = Home::new("cmd-proj");
    let bridge = bridge(&home);
    let storage = storage(&home);
    let root = home.path().join("work");
    fs::create_dir_all(&root).unwrap();
    let store = storage.get().unwrap();
    let project = store
        .add_project("work", &root.display().to_string())
        .unwrap();

    let session = bridge
        .session_create(
            &storage,
            CreateArgs {
                project_id: Some(project.id.clone()),
                cwd: None,
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap();
    bridge.session_stop(session.id, false).await.unwrap();

    assert_eq!(session.cwd, root.display().to_string());
    let metas = store.list_session_meta().unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(metas[0].agent, "custom", "a free-form command is 'custom'");
    assert_eq!(store.get_setting("last_agent").unwrap(), None);
}

#[tokio::test]
async fn a_broken_store_rejects_project_id_but_still_creates_with_cwd() {
    let home = Home::new("cmd-nostore");
    // `goko.db` is a directory, so the store cannot open.
    fs::create_dir_all(home.path().join("goko.db")).unwrap();
    let storage = storage(&home);
    let bridge = bridge(&home);

    let error = bridge
        .session_create(
            &storage,
            CreateArgs {
                project_id: Some("p".into()),
                cwd: None,
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::Storage);
    assert!(home.spawns().is_empty());

    // Daemon-backed session creation still works; the metadata insert is skipped.
    let session = create(&bridge, &storage, "cat").await;
    assert_eq!(session.argv, ["cat"]);
    bridge.session_stop(session.id, false).await.unwrap();
    assert_eq!(home.spawns().len(), 1);
    assert!(bridge.daemon_status().connected);
}

#[tokio::test]
async fn daemon_errors_keep_their_code() {
    let home = Home::new("cmd-err");
    let bridge = bridge(&home);
    let storage = storage(&home);
    let missing = SessionId::new();

    let write = bridge.terminal_write(missing, "x").await.unwrap_err();
    let stop = bridge.session_stop(missing, false).await.unwrap_err();

    assert_eq!(write.code, Code::Daemon(ErrorCode::NotFound));
    assert_eq!(stop.code, Code::Daemon(ErrorCode::NotFound));

    let session = create(&bridge, &storage, "true").await;
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

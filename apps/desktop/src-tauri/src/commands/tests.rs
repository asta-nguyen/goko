use super::*;
use crate::store::Storage;
use crate::test_support::{self, Home};
use protocol::message::ErrorCode;
use protocol::session::{SessionId, SessionState};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

fn bridge(home: &Home) -> Bridge {
    Bridge::new(home.path().to_owned())
}

#[tokio::test]
async fn worktree_operation_lock_coordinates_independent_bridges() {
    let home = Home::new("cmd-wt-cross-instance-lock");
    let first = bridge(&home);
    let second = bridge(&home);
    let launch = first.worktree_operation_guard(false).await.unwrap();
    let remove = tokio::spawn(async move { second.worktree_operation_guard(true).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !remove.is_finished(),
        "removal waits for session launches across bridges"
    );

    drop(launch);
    let remove = tokio::time::timeout(Duration::from_secs(1), remove)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(remove);
}

fn storage(home: &Home) -> Storage {
    Storage::open(home.path())
}

fn git_ok(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn worktree_project(home: &Home, label: &str) -> (Storage, PathBuf, Project) {
    let storage = storage(home);
    let root = home.path().join(label);
    fs::create_dir_all(&root).unwrap();
    git_ok(&root, &["init", "-b", "main"]);
    git_ok(&root, &["config", "user.name", "Goko Test"]);
    git_ok(&root, &["config", "user.email", "goko@example.invalid"]);
    fs::write(root.join("tracked.txt"), "initial\n").unwrap();
    git_ok(&root, &["add", "."]);
    git_ok(&root, &["commit", "-m", "initial"]);
    let canonical_root = fs::canonicalize(&root).unwrap();
    let project = storage
        .get()
        .unwrap()
        .add_project(label, &canonical_root.display().to_string())
        .unwrap();
    (storage, canonical_root, project)
}

fn add_worktree(root: &Path, path: &Path, branch: &str) {
    crate::worktrees::create(
        root,
        &path.display().to_string(),
        "new",
        branch,
        Some("main"),
    )
    .unwrap();
}

fn args(command: &str, cwd: &str) -> CreateArgs {
    CreateArgs {
        command: Some(command.into()),
        agent: None,
        project_id: None,
        checkout: None,
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
        checkout: None,
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

async fn wait_for_cwd(bridge: &Bridge, id: SessionId, wanted: &str) -> SessionInfo {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let sessions = bridge.session_list().await.unwrap();
        let session = sessions
            .into_iter()
            .find(|session| session.id == id)
            .unwrap();
        if session.cwd == wanted {
            return session;
        }
        assert!(
            Instant::now() < deadline,
            "session cwd is {:?}",
            session.cwd
        );
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
async fn deleting_an_exited_session_removes_daemon_and_metadata_records() {
    let home = Home::new("cmd-delete");
    let bridge = bridge(&home);
    let storage = storage(&home);
    let session = create(&bridge, &storage, "sh -c 'exit 0'").await;
    wait_for_state(&bridge, session.id, SessionState::Exited).await;
    let store = storage.get().unwrap();
    assert!(store
        .list_session_meta()
        .unwrap()
        .iter()
        .any(|meta| meta.session_id == session.id.as_uuid().to_string()));

    bridge.session_delete(&storage, session.id).await.unwrap();
    bridge.session_delete(&storage, session.id).await.unwrap();

    assert!(!bridge
        .session_list()
        .await
        .unwrap()
        .iter()
        .any(|item| item.id == session.id));
    assert!(!store
        .list_session_meta()
        .unwrap()
        .iter()
        .any(|meta| meta.session_id == session.id.as_uuid().to_string()));
}

#[tokio::test]
async fn an_older_daemon_delete_error_keeps_session_metadata_for_retry() {
    let home = Home::new("cmd-old-delete");
    let server = test_support::fake_daemon(&home, |_, mut stream| async move {
        let request = test_support::read_request(&mut stream).await.unwrap();
        test_support::send_json(
            &mut stream,
            serde_json::json!({
                "id": request["id"],
                "error": {
                    "code": "malformed",
                    "message": "invalid request: unknown variant `session.delete`, expected one of `system.health`, `system.version`, `events.subscribe`, `session.create`, `session.list`, `session.get`, `session.stop`, `terminal.write`, `terminal.resize`, `terminal.subscribe`, `terminal.unsubscribe`"
                }
            }),
        )
        .await;
    });
    let bridge = bridge(&home);
    let storage = storage(&home);
    let id = SessionId::new();
    storage
        .get()
        .unwrap()
        .insert_session_meta(&SessionMeta {
            session_id: id.as_uuid().to_string(),
            project_id: None,
            agent: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/tmp".into(),
            title: "Shell".into(),
            created_at: 1,
        })
        .unwrap();

    let error = bridge.session_delete(&storage, id).await.unwrap_err();

    assert_eq!(error.code, crate::error::Code::Daemon(ErrorCode::Malformed));
    assert_eq!(
        error.message,
        "Daemon is outdated; save live sessions, restart it, then retry closing this session."
    );
    assert_eq!(storage.get().unwrap().list_session_meta().unwrap().len(), 1);
    server.abort();
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
async fn checkout_launches_are_restricted_to_the_project_root_or_listed_worktrees() {
    let home = Home::new("cmd-checkout");
    let (storage, root, project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/checkout");
    let unrelated = home.path().join("unrelated");
    fs::create_dir(&unrelated).unwrap();
    let bridge = bridge(&home);

    let invalid_cases = [
        CreateArgs {
            checkout: Some(root.display().to_string()),
            ..args("cat", "/tmp")
        },
        CreateArgs {
            project_id: Some(project.id.clone()),
            checkout: Some(unrelated.display().to_string()),
            cwd: None,
            ..args("cat", "/tmp")
        },
        CreateArgs {
            project_id: Some(project.id.clone()),
            checkout: Some(home.path().display().to_string()),
            cwd: None,
            ..args("cat", "/tmp")
        },
    ];
    for request in invalid_cases {
        let error = bridge.session_create(&storage, request).await.unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    }
    assert!(
        home.spawns().is_empty(),
        "invalid checkout requests must not reach the daemon"
    );

    let session = bridge
        .session_create(
            &storage,
            CreateArgs {
                project_id: Some(project.id.clone()),
                checkout: Some(linked.to_string_lossy().into_owned()),
                cwd: None,
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap();
    bridge.session_stop(session.id, false).await.unwrap();
    assert_eq!(
        session.cwd,
        fs::canonicalize(&linked).unwrap().display().to_string()
    );
    let metadata = storage.get().unwrap().list_session_meta().unwrap();
    let meta = metadata
        .iter()
        .find(|item| item.session_id == session.id.as_uuid().to_string())
        .unwrap();
    assert_eq!(meta.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(meta.cwd, session.cwd);

    let root_session = bridge
        .session_create(
            &storage,
            CreateArgs {
                project_id: Some(project.id),
                checkout: Some(root.display().to_string()),
                cwd: None,
                ..args("cat", "/tmp")
            },
        )
        .await
        .unwrap();
    bridge.session_stop(root_session.id, false).await.unwrap();
    assert_eq!(root_session.cwd, root.display().to_string());
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

#[tokio::test]
async fn worktree_removal_guards_root_dirty_and_unlinked_paths_then_preserves_branch() {
    let home = Home::new("cmd-wt-remove");
    let (_storage, root, _project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/keep");
    let bridge = bridge(&home);

    let root_error = crate::worktrees::remove(&bridge, &root, &root.display().to_string())
        .await
        .unwrap_err();
    assert_eq!(root_error.code, Code::Daemon(ErrorCode::InvalidArgument));

    let unrelated = home.path().join("unlinked");
    fs::create_dir(&unrelated).unwrap();
    let unlinked_error = crate::worktrees::remove(&bridge, &root, &unrelated.display().to_string())
        .await
        .unwrap_err();
    assert_eq!(unlinked_error.code, Code::Daemon(ErrorCode::NotFound));

    fs::write(linked.join("untracked.txt"), "dirty\n").unwrap();
    let dirty = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();
    assert!(dirty.message.contains("1 changed files"));
    assert!(linked.is_dir());
    fs::remove_file(linked.join("untracked.txt")).unwrap();

    crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap();
    assert!(!linked.exists());
    let branches =
        String::from_utf8(crate::worktrees::git(&root, &["branch", "--list"]).unwrap()).unwrap();
    assert!(branches
        .lines()
        .any(|branch| branch.trim().trim_start_matches('*').trim() == "feature/keep"));
}

#[tokio::test]
async fn worktree_removal_waits_for_a_session_launch_to_finish_then_rechecks_sessions() {
    let home = Home::new("cmd-wt-launch-race");
    let (storage, root, project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/race");
    let (launch_seen_tx, launch_seen_rx) = oneshot::channel();
    let (allow_launch_tx, allow_launch_rx) = oneshot::channel();
    let (list_seen_tx, mut list_seen_rx) = oneshot::channel();
    let session = SessionInfo {
        id: SessionId::new(),
        title: "Shell".into(),
        argv: vec!["sh".into()],
        cwd: linked.display().to_string(),
        state: SessionState::Running,
        exit_code: None,
        error: None,
        started_at: 1,
        ended_at: None,
    };
    let launch_seen_tx = Arc::new(AsyncMutex::new(Some(launch_seen_tx)));
    let allow_launch_rx = Arc::new(AsyncMutex::new(Some(allow_launch_rx)));
    let list_seen_tx = Arc::new(AsyncMutex::new(Some(list_seen_tx)));
    let session_id = session.id;
    let server_session = session.clone();
    let server = test_support::fake_daemon(&home, move |_, mut stream| {
        let launch_seen_tx = Arc::clone(&launch_seen_tx);
        let allow_launch_rx = Arc::clone(&allow_launch_rx);
        let list_seen_tx = Arc::clone(&list_seen_tx);
        let session = server_session.clone();
        async move {
            let launch_seen_tx = launch_seen_tx.lock().await.take().unwrap();
            let list_seen_tx = list_seen_tx.lock().await.take().unwrap();
            let mut allow_launch_rx = allow_launch_rx.lock().await.take().unwrap();
            let create = test_support::read_request(&mut stream).await.unwrap();
            assert_eq!(create["type"], "session.create");
            launch_seen_tx.send(()).unwrap();

            tokio::select! {
                _ = &mut allow_launch_rx => {
                    test_support::reply(&mut stream, &create, serde_json::to_value(&session).unwrap()).await;
                    let list = test_support::read_request(&mut stream).await.unwrap();
                    assert_eq!(list["type"], "session.list");
                    list_seen_tx.send(()).unwrap();
                    test_support::reply(&mut stream, &list, serde_json::json!({"sessions": [session]})).await;
                }
                list = test_support::read_request(&mut stream) => {
                    let list = list.unwrap();
                    assert_eq!(list["type"], "session.list");
                    list_seen_tx.send(()).unwrap();
                    test_support::reply(&mut stream, &list, serde_json::json!({"sessions": []})).await;
                    let _ = allow_launch_rx.await;
                    test_support::reply(&mut stream, &create, serde_json::to_value(&session).unwrap()).await;
                }
            }
        }
    });
    let bridge = bridge(&home);
    let launch_bridge = bridge.clone();
    let checkout = linked.display().to_string();
    let launch = tokio::spawn(async move {
        launch_bridge
            .session_create(
                &storage,
                CreateArgs {
                    command: Some("sh".into()),
                    agent: None,
                    project_id: Some(project.id),
                    checkout: Some(checkout),
                    cwd: None,
                    title: Some("Shell".into()),
                    cols: 80,
                    rows: 24,
                },
            )
            .await
    });
    launch_seen_rx.await.unwrap();

    let remove_bridge = bridge.clone();
    let remove_root = root.clone();
    let remove_path = linked.display().to_string();
    let remove = tokio::spawn(async move {
        crate::worktrees::remove(&remove_bridge, &remove_root, &remove_path).await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut list_seen_rx)
            .await
            .is_err(),
        "removal must wait until an in-flight launch finishes"
    );

    allow_launch_tx.send(()).unwrap();
    let launched = launch.await.unwrap().unwrap();
    assert_eq!(launched.id, session_id);
    let error = remove.await.unwrap().unwrap_err();
    assert!(error.message.contains("close all running Goko sessions"));
    assert!(linked.is_dir());
    assert!(list_seen_rx.await.is_ok());
    server.abort();
}

#[tokio::test]
async fn worktree_removal_refuses_live_and_exited_sessions_inside_the_checkout() {
    let home = Home::new("cmd-wt-sessions");
    let (storage, root, _project) = worktree_project(&home, "project");
    let live_path = home.path().join("live");
    let exited_path = home.path().join("exited");
    add_worktree(&root, &live_path, "feature/live");
    add_worktree(&root, &exited_path, "feature/exited");
    let bridge = bridge(&home);

    let live = bridge
        .session_create(&storage, args("cat", live_path.to_str().unwrap()))
        .await
        .unwrap();
    let live_error = crate::worktrees::remove(&bridge, &root, &live_path.display().to_string())
        .await
        .unwrap_err();
    assert!(live_error
        .message
        .contains("close all running Goko sessions"));
    bridge.session_stop(live.id, false).await.unwrap();
    wait_for_state(&bridge, live.id, SessionState::Exited).await;
    bridge.session_delete(&storage, live.id).await.unwrap();

    let exited = bridge
        .session_create(
            &storage,
            args("sh -c 'exit 0'", exited_path.to_str().unwrap()),
        )
        .await
        .unwrap();
    wait_for_state(&bridge, exited.id, SessionState::Exited).await;
    let exited_error = crate::worktrees::remove(&bridge, &root, &exited_path.display().to_string())
        .await
        .unwrap_err();
    assert!(exited_error.message.contains("1 attached session"));
    assert!(exited_path.is_dir());
}

#[tokio::test]
async fn worktree_removal_refuses_live_sessions_in_other_projects() {
    let home = Home::new("cmd-wt-global-session");
    let (_storage, root, _) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/global-session");
    let (other_storage, _other_root, other_project) = worktree_project(&home, "other-project");
    let bridge = bridge(&home);
    let live = bridge
        .session_create(
            &other_storage,
            CreateArgs {
                project_id: Some(other_project.id),
                command: Some("cat".into()),
                agent: None,
                checkout: None,
                cwd: None,
                title: None,
                cols: 80,
                rows: 24,
            },
        )
        .await
        .unwrap();

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();
    assert!(error.message.contains("close all running Goko sessions"));
    assert!(linked.is_dir());

    bridge.session_stop(live.id, false).await.unwrap();
    wait_for_state(&bridge, live.id, SessionState::Exited).await;
    bridge
        .session_delete(&other_storage, live.id)
        .await
        .unwrap();
    crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap();
    assert!(!linked.exists());
}

#[tokio::test]
async fn worktree_removal_refuses_a_project_shell_that_cd_into_the_checkout() {
    let home = Home::new("cmd-wt-cd-session");
    let (storage, root, _project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/cd-into-worktree");
    let bridge = bridge(&home);
    let command = format!("sh -c 'cd \"{}\" && exec cat'", linked.display());
    let session = bridge
        .session_create(&storage, args(&command, &root.display().to_string()))
        .await
        .unwrap();
    let linked = fs::canonicalize(linked).unwrap();
    wait_for_cwd(&bridge, session.id, &linked.display().to_string()).await;

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();

    assert!(error.message.contains("close all running Goko sessions"));
    assert!(linked.is_dir());
    bridge.session_stop(session.id, false).await.unwrap();
    wait_for_state(&bridge, session.id, SessionState::Exited).await;
    bridge.session_delete(&storage, session.id).await.unwrap();
}

#[tokio::test]
async fn worktree_removal_fails_closed_when_a_session_cwd_is_unknown() {
    let home = Home::new("cmd-wt-unknown-cwd");
    let (_storage, root, _project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/unknown-cwd");
    let session_id = SessionId::new();
    let session_id_text = session_id.as_uuid().to_string();
    let server = test_support::fake_daemon(&home, move |_, mut stream| {
        let session_id_text = session_id_text.clone();
        async move {
            let request = test_support::read_request(&mut stream).await.unwrap();
            test_support::send_json(
                &mut stream,
                serde_json::json!({
                    "id": request["id"],
                    "result": {"sessions": [{
                        "id": session_id_text,
                        "title": "shell",
                        "argv": ["sh"],
                        "cwd": "",
                        "state": "exited",
                        "exit_code": null,
                        "error": null,
                        "started_at": 1,
                        "ended_at": null
                    }]}
                }),
            )
            .await;
        }
    });
    let bridge = bridge(&home);

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();

    assert!(error
        .message
        .contains("cannot confirm the current directory"));
    assert!(linked.is_dir());
    server.abort();
}

#[tokio::test]
async fn worktree_removal_fails_closed_when_a_session_cwd_is_relative_or_unresolvable() {
    let home = Home::new("cmd-wt-relative-cwd");
    let (_storage, root, _) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/relative-cwd");
    let unresolvable = root.join("tracked.txt").join("child").display().to_string();
    let missing_directory = root.join("missing-directory").display().to_string();
    let server = test_support::fake_daemon(&home, move |_, mut stream| {
        let unresolvable = unresolvable.clone();
        let missing_directory = missing_directory.clone();
        async move {
            for cwd in ["relative/path".to_owned(), unresolvable, missing_directory] {
                let request = test_support::read_request(&mut stream).await.unwrap();
                let session_id_text = SessionId::new().as_uuid().to_string();
                test_support::send_json(
                    &mut stream,
                    serde_json::json!({
                        "id": request["id"],
                        "result": {"sessions": [{
                            "id": session_id_text,
                            "title": "shell",
                            "argv": ["sh"],
                            "cwd": cwd,
                            "state": "exited",
                            "exit_code": 0,
                            "error": null,
                            "started_at": 1,
                            "ended_at": 2
                        }]}
                    }),
                )
                .await;
            }
        }
    });
    let bridge = bridge(&home);

    for _ in 0..3 {
        let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
            .await
            .unwrap_err();
        assert!(error
            .message
            .contains("cannot confirm the current directory"));
        assert!(linked.is_dir());
    }
    server.abort();
}

#[tokio::test]
async fn worktree_removal_refuses_daemons_without_live_cwd_support() {
    let home = Home::new("cmd-wt-old-daemon");
    let (_storage, root, _) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/old-daemon");
    let session_id_text = SessionId::new().as_uuid().to_string();
    let server = test_support::fake_daemon_with_version(&home, "0.1.0", move |_, mut stream| {
        let session_id_text = session_id_text.clone();
        async move {
            let request = test_support::read_request(&mut stream).await.unwrap();
            test_support::send_json(
                &mut stream,
                serde_json::json!({
                    "id": request["id"],
                    "result": {"sessions": [{
                        "id": session_id_text,
                        "title": "shell",
                        "argv": ["sh"],
                        "cwd": "/tmp",
                        "state": "running",
                        "exit_code": null,
                        "error": null,
                        "started_at": 1,
                        "ended_at": null
                    }]}
                }),
            )
            .await;
        }
    });
    let bridge = bridge(&home);

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();

    assert!(error.message.contains("gokod 0.1.0"));
    assert!(error.message.contains("update and restart it manually"));
    assert!(linked.is_dir());
    server.abort();
}

#[tokio::test]
async fn worktree_removal_fails_closed_when_session_list_fails() {
    let home = Home::new("cmd-wt-daemon-fail");
    let (_storage, root, _project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/fail-closed");
    let server = test_support::fake_daemon(&home, |_, mut stream| async move {
        let request = test_support::read_request(&mut stream).await.unwrap();
        test_support::send_json(
            &mut stream,
            serde_json::json!({
                "id": request["id"],
                "error": {"code": "internal", "message": "session list unavailable"}
            }),
        )
        .await;
    });
    let bridge = bridge(&home);

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();

    assert_eq!(error.code, Code::Daemon(ErrorCode::Internal));
    assert!(error.message.contains("session list unavailable"));
    assert!(linked.is_dir());
    server.abort();
}

#[tokio::test]
async fn worktree_removal_fails_closed_when_the_checked_daemon_connection_dies() {
    let home = Home::new("cmd-wt-identity");
    let (_storage, root, _project) = worktree_project(&home, "project");
    let linked = home.path().join("linked");
    add_worktree(&root, &linked, "feature/identity");
    let consulted = Arc::new(AtomicUsize::new(0));
    let consulted_server = Arc::clone(&consulted);
    let server = test_support::fake_daemon(&home, move |index, mut stream| {
        let consulted = Arc::clone(&consulted_server);
        async move {
            if index == 0 {
                drop(stream);
                return;
            }
            consulted.fetch_add(1, Ordering::SeqCst);
            let request = test_support::read_request(&mut stream).await.unwrap();
            test_support::reply(&mut stream, &request, serde_json::json!({"sessions": []})).await;
        }
    });
    let bridge = bridge(&home);

    let error = crate::worktrees::remove(&bridge, &root, &linked.display().to_string())
        .await
        .unwrap_err();

    assert_eq!(error.code, Code::Disconnected);
    assert!(linked.is_dir());
    assert_eq!(
        consulted.load(Ordering::SeqCst),
        0,
        "a replacement daemon must not answer the session list"
    );
    server.abort();
}

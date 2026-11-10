use daemon::session::{CreateSpec, SessionError, SessionManager};
use protocol::session::{SessionId, SessionInfo, SessionState};
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::time::{sleep, timeout};

fn spec(argv: &[&str]) -> CreateSpec {
    CreateSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir()
            .expect("current directory")
            .to_string_lossy()
            .into_owned(),
        cols: 80,
        rows: 24,
        title: String::new(),
    }
}

/// The next event for session `id`, skipping other sessions' events.
async fn next_event(events: &mut Receiver<SessionInfo>, id: SessionId) -> SessionInfo {
    timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the event stream stays open");
            if event.id == id {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for a session event")
}

async fn next_event_with_state(
    events: &mut Receiver<SessionInfo>,
    id: SessionId,
    state: SessionState,
) -> SessionInfo {
    loop {
        let event = next_event(events, id).await;
        if event.state == state {
            return event;
        }
    }
}

async fn wait_for_state(manager: &SessionManager, id: SessionId, state: SessionState) {
    for _ in 0..500 {
        if manager.get(id).expect("session exists").state == state {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("session never reached {state:?}");
}

async fn wait_for_output(manager: &SessionManager, id: SessionId, needle: &str) {
    for _ in 0..500 {
        let (_, bytes, _) = manager.pty(id).expect("session has a process").buffer();
        if String::from_utf8_lossy(&bytes).contains(needle) {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("output never contained {needle:?}");
}

fn output(manager: &SessionManager, id: SessionId) -> String {
    let (_, bytes, _) = manager.pty(id).expect("session has a process").buffer();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[tokio::test]
async fn a_cat_session_runs_and_events_follow_the_state_machine() {
    let manager = SessionManager::new();
    let mut events = manager.subscribe_events();

    let created = manager.create(spec(&["cat"])).expect("create cat");
    assert_eq!(created.state, SessionState::Running);
    assert_eq!(created.title, "cat");
    assert_eq!(created.exit_code, None);
    assert_eq!(manager.list(), vec![created.clone()]);
    assert_eq!(manager.get(created.id).unwrap(), created);

    let starting = next_event(&mut events, created.id).await;
    let running = next_event(&mut events, created.id).await;
    assert_eq!(starting.state, SessionState::Starting);
    assert_eq!(running.state, SessionState::Running);

    let stopped = manager.stop(created.id, false).expect("stop cat");
    assert_eq!(stopped.state, SessionState::Stopping);
    let stopping = next_event_with_state(&mut events, created.id, SessionState::Stopping).await;
    let exited = next_event_with_state(&mut events, created.id, SessionState::Exited).await;
    assert_eq!(stopping.state, SessionState::Stopping);
    assert_eq!(exited.state, SessionState::Exited);
    assert_eq!(
        exited.exit_code, None,
        "killed by a signal has no exit code"
    );
    assert!(exited.ended_at.unwrap() >= exited.started_at);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_shell_directory_change_updates_session_list_get_and_events() {
    let directory = std::env::temp_dir().join(format!("goko-cwd-{}", SessionId::new().as_uuid()));
    fs::create_dir(&directory).expect("create target directory");
    let expected = fs::canonicalize(&directory)
        .expect("canonical target directory")
        .to_string_lossy()
        .into_owned();
    let manager = SessionManager::new();
    let mut events = manager.subscribe_events();
    let mut create = spec(&["sh"]);
    create.argv = ["sh", "-c", "cd \"$1\" && exec cat", "goko-cwd", &expected]
        .into_iter()
        .map(str::to_owned)
        .collect();

    let session = manager.create(create).expect("create shell");
    let changed = timeout(Duration::from_secs(5), async {
        loop {
            let info = events
                .recv()
                .await
                .expect("session event stream stays open");
            if info.id == session.id && info.cwd == expected {
                break info;
            }
        }
    })
    .await
    .expect("live CWD update event");
    assert_eq!(changed.cwd, expected);
    assert_eq!(manager.get(session.id).unwrap().cwd, expected);
    assert_eq!(manager.list()[0].cwd, expected);

    manager.stop(session.id, false).unwrap();
    wait_for_state(&manager, session.id, SessionState::Exited).await;
    assert_eq!(manager.get(session.id).unwrap().cwd, expected);
    fs::remove_dir_all(directory).expect("remove target directory");
}

#[tokio::test]
async fn exit_code_title_and_creation_order_are_recorded() {
    let manager = SessionManager::new();
    let mut first = spec(&["sh", "-c", "exit 7"]);
    first.title = "  build  ".to_owned();
    let first = manager.create(first).expect("create first");
    let second = manager.create(spec(&["/bin/cat"])).expect("create second");

    wait_for_state(&manager, first.id, SessionState::Exited).await;

    let listed = manager.list();
    assert_eq!(
        listed.iter().map(|info| info.id).collect::<Vec<_>>(),
        [first.id, second.id]
    );
    assert_eq!(listed[0].title, "build");
    assert_eq!(listed[0].exit_code, Some(7));
    assert_eq!(listed[1].title, "cat", "title defaults to the basename");
    manager.stop(second.id, true).unwrap();
}

#[tokio::test]
async fn stop_is_idempotent_and_sends_one_signal() {
    let manager = SessionManager::new();
    let script = "trap 'echo got-term' TERM; echo ready; while true; do sleep 0.05; done";
    let session = manager.create(spec(&["sh", "-c", script])).unwrap();
    wait_for_output(&manager, session.id, "ready").await;

    // Two simultaneous stops and a late third one.
    std::thread::scope(|scope| {
        scope.spawn(|| manager.stop(session.id, false).unwrap());
        scope.spawn(|| manager.stop(session.id, false).unwrap());
    });
    wait_for_output(&manager, session.id, "got-term").await;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(
        manager.stop(session.id, false).unwrap().state,
        SessionState::Stopping
    );
    sleep(Duration::from_millis(300)).await;
    assert_eq!(output(&manager, session.id).matches("got-term").count(), 1);

    // The script keeps running; a forced stop escalates to SIGKILL.
    manager.stop(session.id, true).unwrap();
    wait_for_state(&manager, session.id, SessionState::Exited).await;
}

#[tokio::test]
async fn stopping_an_exited_session_sends_nothing() {
    let manager = SessionManager::new();
    let session = manager.create(spec(&["sh", "-c", "exit 0"])).unwrap();
    wait_for_state(&manager, session.id, SessionState::Exited).await;
    let mut events = manager.subscribe_events();

    for force in [false, true] {
        let info = manager.stop(session.id, force).expect("stop after exit");
        assert_eq!(info.state, SessionState::Exited);
        assert_eq!(info.exit_code, Some(0));
    }
    assert!(events.try_recv().is_err(), "no event for a no-op stop");
}

#[tokio::test]
async fn unknown_ids_are_not_found() {
    let manager = SessionManager::new();
    let id = SessionId::new();

    assert!(matches!(manager.get(id), Err(SessionError::NotFound(_))));
    assert!(matches!(
        manager.stop(id, false),
        Err(SessionError::NotFound(_))
    ));
    assert!(matches!(manager.pty(id), Err(SessionError::NotFound(_))));
}

#[tokio::test]
async fn deletion_rejects_live_sessions_and_removes_finished_and_failed_sessions() {
    let manager = SessionManager::new();
    let live = manager.create(spec(&["cat"])).unwrap();

    assert!(matches!(
        manager.delete(live.id),
        Err(SessionError::InvalidArgument(_))
    ));
    assert_eq!(manager.list().len(), 1);

    manager.stop(live.id, false).unwrap();
    wait_for_state(&manager, live.id, SessionState::Exited).await;
    manager.delete(live.id).unwrap();
    assert!(manager.list().is_empty());
    assert!(matches!(
        manager.get(live.id),
        Err(SessionError::NotFound(_))
    ));
    assert!(matches!(
        manager.pty(live.id),
        Err(SessionError::NotFound(_))
    ));
    manager.delete(live.id).unwrap();

    let mut bad_cwd = spec(&["cat"]);
    bad_cwd.cwd = "/definitely/not/a/directory".to_owned();
    let failed = manager.create(bad_cwd).unwrap();
    assert_eq!(failed.state, SessionState::Failed);
    manager.delete(failed.id).unwrap();
    assert!(manager.list().is_empty());
}

#[tokio::test]
async fn spawn_failures_become_failed_sessions() {
    let manager = SessionManager::new();
    let mut events = manager.subscribe_events();

    let mut bad_cwd = spec(&["sh"]);
    bad_cwd.cwd = "/definitely/not/a/directory".to_owned();
    let failed = manager
        .create(bad_cwd)
        .expect("a failed session, not an error");
    assert_eq!(failed.state, SessionState::Failed);
    assert!(failed
        .error
        .as_deref()
        .unwrap()
        .contains("working directory"));
    assert!(failed.ended_at.is_some());

    let missing = manager
        .create(spec(&["/definitely-not-a-goko-command"]))
        .expect("a failed session, not an error");
    assert_eq!(missing.state, SessionState::Failed);
    assert!(missing.error.is_some());

    assert_eq!(manager.list().len(), 2, "failed sessions stay listed");
    assert_eq!(
        next_event(&mut events, failed.id).await.state,
        SessionState::Starting
    );
    assert_eq!(
        next_event(&mut events, failed.id).await.state,
        SessionState::Failed
    );
    assert!(matches!(
        manager.pty(failed.id),
        Err(SessionError::InvalidArgument(_))
    ));
    assert_eq!(
        manager.stop(failed.id, false).unwrap().state,
        SessionState::Failed
    );
}

#[tokio::test]
async fn invalid_input_is_rejected_without_creating_a_session() {
    let manager = SessionManager::new();

    let mut zero = spec(&["sh"]);
    zero.cols = 0;
    let mut huge = spec(&["sh"]);
    huge.rows = 1001;
    for bad in [spec(&[]), spec(&[""]), zero, huge] {
        assert!(matches!(
            manager.create(bad),
            Err(SessionError::InvalidArgument(_))
        ));
    }
    assert!(manager.list().is_empty());
}

#[tokio::test]
async fn two_sessions_have_independent_output() {
    let manager = SessionManager::new();
    let a = manager
        .create(spec(&["sh", "-c", "echo alpha-only"]))
        .unwrap();
    let b = manager
        .create(spec(&["sh", "-c", "echo bravo-only"]))
        .unwrap();
    wait_for_output(&manager, a.id, "alpha-only").await;
    wait_for_output(&manager, b.id, "bravo-only").await;

    assert!(!output(&manager, a.id).contains("bravo"));
    assert!(!output(&manager, b.id).contains("alpha"));
}

#[tokio::test]
async fn shutdown_stops_every_process_even_one_that_ignores_sigterm() {
    let manager = Arc::new(SessionManager::new());
    let mut ids = vec![];
    for _ in 0..3 {
        ids.push(manager.create(spec(&["sleep", "30"])).unwrap().id);
    }
    let stubborn = "trap '' TERM; echo ready; while true; do sleep 1; done";
    let stubborn = manager.create(spec(&["sh", "-c", stubborn])).unwrap().id;
    wait_for_output(&manager, stubborn, "ready").await;
    ids.push(stubborn);
    let pids: Vec<u32> = ids
        .iter()
        .map(|id| manager.pty(*id).unwrap().pid())
        .collect();

    let started = Instant::now();
    manager.shutdown().await;

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    for id in &ids {
        manager.pty(*id).unwrap().wait_exit().await.expect("exited");
    }
    for pid in pids {
        assert!(!process_exists(pid), "process {pid} survived shutdown");
    }
}

#[tokio::test]
async fn shutdown_without_sessions_returns_immediately() {
    let manager = SessionManager::new();
    let started = Instant::now();

    manager.shutdown().await;

    assert!(started.elapsed() < Duration::from_millis(200));
}

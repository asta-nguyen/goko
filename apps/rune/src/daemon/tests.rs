//! The daemon client's own tests: a stand-in `gokod` on a Unix socket.
//!
//! The fake completes the handshake, then hands every request to the
//! test's handler, so a test can answer in its own order, push events or
//! flood the output queue.

use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use protocol::frame::{self, Frame, FrameKind};
use protocol::message::{Event, Request};
use protocol::session::{SessionId, SessionInfo, SessionState};
use serde_json::{Value, json};

use super::*;

/// Every request the fake reads: the JSON message and the connection it
/// arrived on.
type Handler = Arc<dyn Fn(Value, &mut UnixStream) + Send + Sync>;

/// A stand-in daemon: it completes the handshake and then runs the
/// handler for every request the client sends.
struct FakeDaemon {
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeDaemon {
    fn start(home: PathBuf, handler: Handler) -> FakeDaemon {
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("daemon.token"), "tok").unwrap();
        let listener = UnixListener::bind(home.join("daemon.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        serve_connection(stream, &handler);
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        FakeDaemon {
            connections,
            stop,
            thread: Some(thread),
        }
    }

    /// How many connections the client opened so far.
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        // The listener stops accepting, so the socket closes and the client
        // sees the daemon gone.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Complete the handshake, then run the handler for every request.
fn serve_connection(mut stream: UnixStream, handler: &Handler) {
    let hello = read_message(&mut stream).expect("the client sent a handshake");
    assert_eq!(hello["token"], "tok");
    write_json(&mut stream, &json!({ "daemon_version": "0.1.2" }));
    while let Some(message) = read_message(&mut stream) {
        handler(message, &mut stream);
    }
}

/// The handler every test starts from: it answers the bootstrap requests
/// and echoes `{}` for anything else.
fn default_handler(sessions: Vec<SessionInfo>) -> Handler {
    Arc::new(move |message: Value, stream: &mut UnixStream| {
        let id = message["id"].clone();
        let result = match message["type"].as_str().unwrap_or_default() {
            "session.list" => json!({ "sessions": sessions }),
            _ => json!({}),
        };
        write_json(stream, &json!({ "id": id, "result": result }));
    })
}

/// The reply a handler sends for `message`: the client's bootstrap needs the
/// sessions array, everything else answers `{}`.
fn reply_for(message: &Value) -> Value {
    let result = match message["type"].as_str().unwrap_or_default() {
        "session.list" => json!({ "sessions": [] }),
        _ => json!({}),
    };
    json!({ "id": message["id"].clone(), "result": result })
}

/// A temporary `GOKO_HOME`; removed when the test ends.
struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> TempHome {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "goko-rune-daemon-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempHome(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The events the client reported so far.
#[derive(Default)]
struct Events {
    seen: Vec<UiEvent>,
}

impl Events {
    /// Wait for the first event that matches, keeping the others.
    fn wait_for(&mut self, daemon: &Daemon, predicate: impl Fn(&UiEvent) -> bool) -> UiEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.seen.extend(daemon.take_events());
            if let Some(found) = self.seen.iter().find(|event| predicate(event)) {
                return found.clone();
            }
            assert!(
                Instant::now() < deadline,
                "the event did not arrive in time: {:?}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn session_info(state: SessionState) -> SessionInfo {
    SessionInfo {
        id: SessionId::new(),
        title: "shell".into(),
        argv: vec!["zsh".into()],
        cwd: "/tmp".into(),
        state,
        exit_code: None,
        error: None,
        started_at: 1_700_000_000_000,
        ended_at: None,
    }
}

fn read_message(stream: &mut UnixStream) -> Option<Value> {
    let mut header = [0_u8; 5];
    stream.read_exact(&mut header).ok()?;
    let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).ok()?;
    Some(serde_json::from_slice(&payload).unwrap())
}

fn write_json(stream: &mut UnixStream, message: &Value) {
    let bytes = frame::encode(&Frame {
        kind: FrameKind::Control,
        payload: serde_json::to_vec(message).unwrap(),
    })
    .unwrap();
    stream.write_all(&bytes).unwrap();
}

fn write_output(stream: &mut UnixStream, session: SessionId, seq: u64, data: &[u8]) {
    let payload = frame::encode_output(*session.as_bytes(), seq, data).unwrap();
    let bytes = frame::encode(&Frame {
        kind: FrameKind::TerminalOutput,
        payload,
    })
    .unwrap();
    stream.write_all(&bytes).unwrap();
}

#[test]
fn a_connected_client_reports_the_version_and_the_session_list() {
    let home = TempHome::new("list");
    let session = session_info(SessionState::Running);
    let _fake = FakeDaemon::start(home.path(), default_handler(vec![session.clone()]));
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let mut events = Events::default();

    let connected = events.wait_for(&daemon, |event| matches!(event, UiEvent::Connected { .. }));
    assert!(matches!(
        connected,
        UiEvent::Connected { version } if version == "0.1.2"
    ));

    let listed = events.wait_for(&daemon, |event| matches!(event, UiEvent::Sessions(_)));
    match listed {
        UiEvent::Sessions(sessions) => {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session.id);
        }
        other => panic!("expected the session list, got {other:?}"),
    }
    assert_eq!(daemon.daemon_version().as_deref(), Some("0.1.2"));
    assert!(daemon.supports_flow_control());
}

#[test]
fn every_request_is_answered_with_its_own_reply() {
    let home = TempHome::new("ids");
    let pending: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let handler = {
        let pending = Arc::clone(&pending);
        Arc::new(move |message: Value, stream: &mut UnixStream| {
            let kind = message["type"].as_str().unwrap_or_default().to_string();
            if kind == "system.health" || kind == "system.version" {
                // Answer both requests in the opposite order: the ids must
                // still carry each reply to its own caller.
                let mut pending = pending.lock().unwrap();
                pending.push(message);
                if pending.len() == 2 {
                    for request in pending.drain(..).rev() {
                        write_json(
                            stream,
                            &json!({ "id": request["id"], "result": { "echo": request["type"] } }),
                        );
                    }
                }
                return;
            }
            write_json(stream, &reply_for(&message));
        })
    };
    let _fake = FakeDaemon::start(home.path(), handler);
    let daemon = Daemon::spawn_connect_only(home.path(), || {});

    let health = daemon.call(|id| Request::SystemHealth { id }).unwrap();
    let version = daemon.call(|id| Request::SystemVersion { id }).unwrap();

    let health = health
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let version = version
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_ne!(health.id, version.id);
    assert_eq!(result_of(&health).unwrap()["echo"], "system.health");
    assert_eq!(result_of(&version).unwrap()["echo"], "system.version");
}

#[test]
fn a_typed_answer_is_parsed_off_the_ui_thread() {
    let home = TempHome::new("json");
    let session = session_info(SessionState::Running);
    let _fake = FakeDaemon::start(home.path(), default_handler(vec![session.clone()]));
    let daemon = Daemon::spawn_connect_only(home.path(), || {});

    let list = daemon
        .call_json::<SessionList>(|id| Request::SessionList { id })
        .unwrap();
    let list = list.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();

    assert_eq!(list.sessions.len(), 1);
    assert_eq!(list.sessions[0].id, session.id);
}

#[test]
fn a_daemon_event_reaches_the_ui() {
    let home = TempHome::new("events");
    let session = session_info(SessionState::Running);
    let handler = {
        let session = session.clone();
        Arc::new(move |message: Value, stream: &mut UnixStream| {
            if message["type"].as_str() == Some("events.subscribe") {
                // The event arrives before the reply, like the daemon's
                // own event stream does.
                write_json(
                    stream,
                    &json!({ "type": "session.updated", "session": session }),
                );
            }
            write_json(stream, &reply_for(&message));
        })
    };
    let _fake = FakeDaemon::start(home.path(), handler);
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let mut events = Events::default();

    let event = events.wait_for(&daemon, |event| {
        matches!(event, UiEvent::Event(Event::SessionUpdated { .. }))
    });
    match event {
        UiEvent::Event(Event::SessionUpdated { session }) => {
            assert_eq!(session.id, session.id);
            assert_eq!(session.state, SessionState::Running);
            assert_eq!(session.title, "shell");
        }
        other => panic!("expected the session update, got {other:?}"),
    }
}

#[test]
fn terminal_output_reaches_the_subscriber_of_its_session() {
    let home = TempHome::new("output");
    let handler = Arc::new(move |message: Value, stream: &mut UnixStream| {
        if message["type"].as_str() == Some("terminal.subscribe") {
            let session: SessionId = serde_json::from_value(message["session_id"].clone()).unwrap();
            write_output(stream, session, 7, b"hello");
            write_output(stream, session, 9, b"!");
        }
        write_json(stream, &reply_for(&message));
    });
    let _fake = FakeDaemon::start(home.path(), handler);
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let session = SessionId::new();

    let (chunks, ready) = daemon.attach_output(session, false).unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();

    let first = chunks.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.seq, 7);
    assert_eq!(first.data, b"hello".to_vec());
    let second = chunks.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(second.seq, 9);
    assert_eq!(second.data, b"!".to_vec());
}

#[test]
fn a_consumer_that_falls_behind_loses_the_subscription_and_is_told() {
    let home = TempHome::new("lag");
    let handler = Arc::new(move |message: Value, stream: &mut UnixStream| {
        if message["type"].as_str() == Some("terminal.subscribe") {
            let session: SessionId = serde_json::from_value(message["session_id"].clone()).unwrap();
            for seq in 0..(OUTPUT_QUEUE as u64 + 20) {
                write_output(stream, session, seq, b"x");
            }
        }
        write_json(stream, &reply_for(&message));
    });
    let _fake = FakeDaemon::start(home.path(), handler);
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let session = SessionId::new();
    let mut events = Events::default();

    let (chunks, ready) = daemon.attach_output(session, false).unwrap();
    // The reply comes after the frames, so the queue is already full when
    // the consumer starts reading.
    ready.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();

    // The consumer does not read while the frames arrive: the queue fills
    // up and the subscription ends instead of stalling the reader.
    let mut delivered = 0;
    while chunks.recv_timeout(Duration::from_secs(5)).is_ok() {
        delivered += 1;
    }
    assert_eq!(delivered, OUTPUT_QUEUE);

    let lagged = events.wait_for(&daemon, |event| {
        matches!(event, UiEvent::Event(Event::TerminalLagged { .. }))
    });
    match lagged {
        UiEvent::Event(Event::TerminalLagged { session_id }) => {
            assert_eq!(session_id, session);
        }
        other => panic!("expected the lag notice, got {other:?}"),
    }
}

#[test]
fn a_lost_connection_is_reported_and_the_client_redials() {
    let home = TempHome::new("redial");
    let handler = Arc::new(move |message: Value, stream: &mut UnixStream| {
        if message["type"].as_str() == Some("session.list") {
            // The daemon goes away right after answering.
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        write_json(stream, &reply_for(&message));
    });
    let fake = FakeDaemon::start(home.path(), handler);
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let mut events = Events::default();

    let disconnected = events.wait_for(&daemon, |event| {
        matches!(event, UiEvent::Disconnected { .. })
    });
    assert!(matches!(disconnected, UiEvent::Disconnected { .. }));

    // The supervisor redials: the fake accepts the next connection.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.connections() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fake.connections(), 2);
    // The redial reconnects and reports itself again.
    events.wait_for(&daemon, |event| matches!(event, UiEvent::Connected { .. }));
}

#[test]
fn a_request_made_while_the_daemon_is_gone_fails_instead_of_hanging() {
    let home = TempHome::new("gone");
    // Nobody listens on the socket: the client must answer the request
    // instead of leaving it pending.
    let daemon = Daemon::spawn_connect_only(home.path(), || {});

    let reply = daemon.call(|id| Request::SystemHealth { id }).unwrap();
    let outcome = reply.recv_timeout(Duration::from_secs(5)).unwrap();
    match outcome {
        Err(error) => assert_eq!(error.code, Code::Disconnected),
        Ok(_) => panic!("the request should not have been answered"),
    }
}

#[test]
fn the_connect_only_mode_never_starts_a_daemon() {
    let home = TempHome::new("nospawn");
    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let mut events = Events::default();

    let disconnected = events.wait_for(&daemon, |event| {
        matches!(event, UiEvent::Disconnected { .. })
    });
    assert!(matches!(disconnected, UiEvent::Disconnected { .. }));
    assert!(
        !home.path().join("daemon.log").exists(),
        "the client must not start a daemon in this mode"
    );
}

#[test]
fn the_daemon_binary_comes_from_the_environment_or_the_executable() {
    let exe = Path::new("/opt/goko/bin/goko-rune");
    assert_eq!(
        daemon_bin_from(Some("/opt/goko/bin/gokod".into()), exe),
        PathBuf::from("/opt/goko/bin/gokod")
    );
    assert_eq!(
        daemon_bin_from(Some(String::new().into()), exe),
        PathBuf::from("/opt/goko/bin/gokod")
    );
    assert_eq!(
        daemon_bin_from(None, exe),
        PathBuf::from("/opt/goko/bin/gokod")
    );
}

#[test]
fn the_flow_control_flag_follows_the_daemon_version() {
    assert!(!version_at_least("0.1.1", (0, 1, 2)));
    assert!(version_at_least("0.1.2", (0, 1, 2)));
    assert!(version_at_least("0.2.0", (0, 1, 2)));
    assert!(version_at_least("1.0.0", (0, 1, 2)));
    assert!(!version_at_least("unknown", (0, 1, 2)));
    assert!(!version_at_least("0.1.2-rc.1", (0, 1, 2)));
}

/// The `gokod` binary beside the test executable, when the workspace built
/// it. `None` when only this package was built.
fn gokod_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = exe.parent()?.parent()?.join("gokod");
    bin.exists().then_some(bin)
}

/// The whole way through: the real daemon in a temp `GOKO_HOME`, a session
/// started through the client, and its output coming back.
#[test]
fn a_real_daemon_serves_a_session() {
    let Some(bin) = gokod_binary() else {
        eprintln!("skipping: gokod is not built (run the workspace tests)");
        return;
    };
    let home = TempHome::new("real");
    let mut child = std::process::Command::new(&bin)
        .env("GOKO_HOME", home.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start gokod");
    // The daemon writes its socket when it is ready.
    let socket = home.path().join("daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(socket.exists(), "the daemon did not bind its socket");

    let daemon = Daemon::spawn_connect_only(home.path(), || {});
    let mut events = Events::default();
    events.wait_for(&daemon, |event| matches!(event, UiEvent::Connected { .. }));

    // A shell session in the temp home.
    let cwd = home.path().display().to_string();
    let created = daemon
        .call_json::<protocol::session::SessionInfo>(move |id| Request::SessionCreate {
            id,
            argv: vec!["/bin/sh".into()],
            cwd: cwd.clone(),
            cols: 80,
            rows: 24,
            title: String::new(),
        })
        .unwrap();
    let session = created
        .recv_timeout(Duration::from_secs(30))
        .expect("the daemon answered")
        .expect("the daemon created the session");

    // Its output comes back through the kind-1 frames.
    let (mut chunks, ready) = daemon
        .attach_output(session.id, false)
        .expect("subscribe to the session");
    ready
        .recv_timeout(Duration::from_secs(30))
        .expect("the daemon answered the subscribe")
        .expect("the subscribe succeeded");

    let write = daemon
        .call(move |id| Request::TerminalWrite {
            id,
            session_id: session.id,
            data_b64: {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode("echo goko-ready\n")
            },
        })
        .unwrap();
    write
        .recv_timeout(Duration::from_secs(30))
        .expect("the daemon answered the write")
        .expect("the write succeeded");

    // The echo comes back on the output stream.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    while !seen.contains("goko-ready") && Instant::now() < deadline {
        match chunks.recv_timeout(Duration::from_secs(1)) {
            Ok(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk.data)),
            Err(_) => continue,
        }
    }
    assert!(
        seen.contains("goko-ready"),
        "the session's output never came back: {seen:?}"
    );

    // The session can be stopped and deleted through the client.
    let stopped = daemon
        .call_json::<protocol::session::SessionInfo>(move |id| Request::SessionStop {
            id,
            session_id: session.id,
            force: false,
        })
        .unwrap();
    stopped
        .recv_timeout(Duration::from_secs(30))
        .expect("the daemon answered the stop")
        .expect("the stop succeeded");
    // The daemon refuses a delete before the process is really gone. An
    // interactive shell ignores SIGTERM, so the force kill is what ends
    // it — the same escalation the app offers.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut forced = false;
    loop {
        let asked = daemon
            .call_json::<protocol::session::SessionInfo>(move |id| Request::SessionGet {
                id,
                session_id: session.id,
            })
            .unwrap();
        let info = asked
            .recv_timeout(Duration::from_secs(5))
            .expect("the daemon answered")
            .expect("the session exists");
        if info.state == protocol::session::SessionState::Exited {
            break;
        }
        if !forced {
            forced = true;
            let killed = daemon
                .call_json::<protocol::session::SessionInfo>(move |id| Request::SessionStop {
                    id,
                    session_id: session.id,
                    force: true,
                })
                .unwrap();
            killed
                .recv_timeout(Duration::from_secs(5))
                .expect("the daemon answered the force kill")
                .expect("the force kill succeeded");
        }
        assert!(
            Instant::now() < deadline,
            "the session never exited: {:?}",
            info.state
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deleted = daemon
        .call_json::<serde_json::Value>(move |id| Request::SessionDelete {
            id,
            session_id: session.id,
        })
        .unwrap();
    deleted
        .recv_timeout(Duration::from_secs(30))
        .expect("the daemon answered the delete")
        .expect("the delete succeeded");

    let _ = child.kill();
    let _ = child.wait();
}

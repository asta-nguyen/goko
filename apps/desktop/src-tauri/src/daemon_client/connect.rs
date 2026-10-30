use super::*;
use crate::test_support::{fake_daemon, read_json, send_json, send_output, Home};
use serde_json::json;
use tokio::net::UnixListener;

fn health(id: u64) -> Request {
    Request::SystemHealth { id }
}

fn code_of(result: Result<DaemonClient, BridgeError>) -> Code {
    result.err().expect("expected an error").code
}

#[tokio::test]
async fn correct_token_connects_and_health_succeeds() {
    let home = Home::new("ok");
    home.start_daemon();

    let client = DaemonClient::connect(home.path()).await.unwrap();
    let reply = client.request(health).await.unwrap();

    assert_eq!(reply["status"], "ok");
    // A second request on the same connection gets its own id and reply.
    assert_eq!(client.request(health).await.unwrap()["status"], "ok");
}

#[tokio::test]
async fn daemon_errors_pass_through_with_their_code() {
    let home = Home::new("err");
    home.start_daemon();
    let client = DaemonClient::connect(home.path()).await.unwrap();

    // The daemon answers `not_found` for requests it does not implement.
    let session = SessionId::new();
    let error = client
        .request(|id| Request::SessionGet {
            id,
            session_id: session,
        })
        .await;

    assert_eq!(error.err().unwrap().code, Code::Daemon(ErrorCode::NotFound));
}

#[tokio::test]
async fn corrupted_token_file_is_auth_failed() {
    let home = Home::new("tok");
    home.start_daemon();
    fs::write(home.path().join("daemon.token"), "0".repeat(64)).unwrap();

    let result = DaemonClient::connect(home.path()).await;

    assert_eq!(code_of(result), Code::Daemon(ErrorCode::AuthFailed));
}

#[tokio::test]
async fn no_daemon_is_unavailable() {
    let home = Home::new("none");
    assert_eq!(
        code_of(DaemonClient::connect(home.path()).await),
        Code::DaemonUnavailable
    );

    // A stale token file without a socket changes nothing.
    fs::create_dir_all(home.path()).unwrap();
    fs::write(home.path().join("daemon.token"), "0".repeat(64)).unwrap();
    assert_eq!(
        code_of(DaemonClient::connect(home.path()).await),
        Code::DaemonUnavailable
    );
}

#[tokio::test]
async fn socket_without_token_file_is_unavailable() {
    let home = Home::new("notok");
    fs::create_dir_all(home.path()).unwrap();
    let _listener = UnixListener::bind(home.path().join("daemon.sock")).unwrap();

    assert_eq!(
        code_of(DaemonClient::connect(home.path()).await),
        Code::DaemonUnavailable
    );
}

#[tokio::test]
async fn killing_the_daemon_mid_request_is_disconnected() {
    let home = Home::new("kill");
    home.start_daemon();
    let client = Arc::new(DaemonClient::connect(home.path()).await.unwrap());

    // A stopped daemon never answers, so the request stays in flight until it dies.
    home.signal_daemons(libc::SIGSTOP);
    let waiting = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.request(health).await }
    });
    sleep(Duration::from_millis(100)).await;
    home.signal_daemons(libc::SIGKILL);

    let error = waiting.await.unwrap().err().unwrap();
    assert_eq!(error.code, Code::Disconnected);
    // The connection stays dead: later requests and subscriptions fail at once.
    assert_eq!(
        client.request(health).await.err().unwrap().code,
        Code::Disconnected
    );
    assert_eq!(client.events().err().unwrap().code, Code::Disconnected);
}

#[tokio::test]
async fn unanswered_request_times_out() {
    let home = Home::new("slow");
    home.start_daemon();
    let client = DaemonClient::connect(home.path()).await.unwrap();

    home.signal_daemons(libc::SIGSTOP);
    let error = client
        .request_within(Duration::from_millis(200), health)
        .await
        .err()
        .unwrap();

    assert_eq!(error.code, Code::Timeout);
}

#[tokio::test]
async fn frames_are_routed_by_kind_session_and_response_id() {
    let home = Home::new("route");
    let session = SessionId::new();
    let other = SessionId::new();
    let _server = fake_daemon(&home, move |_, mut stream| async move {
        // Two requests in flight; answer them in reverse order, after an event and
        // output for two sessions that are interleaved with the replies.
        let first = read_json(&mut stream).await;
        let second = read_json(&mut stream).await;
        let exited = json!({"type": "session.exited", "session_id": session, "exit_code": 3});
        send_json(&mut stream, exited).await;
        send_output(&mut stream, session, 7, b"hi").await;
        send_output(&mut stream, other, 99, b"nobody listens").await;
        send_output(&mut stream, session, 9, b"!").await;
        for request in [second, first] {
            let echo = json!({"id": request["id"], "result": {"type": request["type"]}});
            send_json(&mut stream, echo).await;
        }
    });
    let client = DaemonClient::connect(home.path()).await.unwrap();
    let mut events = client.events().unwrap();
    let mut output = client.subscribe_output(session).unwrap();

    let (a, b) = tokio::join!(
        client.request(health),
        client.request(|id| Request::SystemVersion { id })
    );

    assert_eq!(a.unwrap()["type"], "system.health");
    assert_eq!(b.unwrap()["type"], "system.version");
    assert_eq!(
        events.recv().await.unwrap(),
        Event::SessionExited {
            session_id: session,
            exit_code: Some(3)
        }
    );
    assert_eq!(
        output.recv().await.unwrap(),
        Output {
            seq: 7,
            data: b"hi".to_vec()
        }
    );
    assert_eq!(
        output.recv().await.unwrap(),
        Output {
            seq: 9,
            data: b"!".to_vec()
        }
    );
}

#[tokio::test]
async fn a_full_output_queue_ends_that_subscription_but_not_the_connection() {
    let home = Home::new("lag");
    let session = SessionId::new();
    let _server = fake_daemon(&home, move |_, mut stream| async move {
        let request = read_json(&mut stream).await;
        for seq in 0..OUTPUT_QUEUE as u64 + 20 {
            send_output(&mut stream, session, seq, b"x").await;
        }
        send_json(&mut stream, json!({"id": request["id"], "result": {}})).await;
    });
    let client = DaemonClient::connect(home.path()).await.unwrap();
    let mut output = client.subscribe_output(session).unwrap();

    // The consumer does not read, yet the reply still arrives: the reader never waits.
    client.request(health).await.unwrap();

    let mut delivered = 0;
    while output.recv().await.is_some() {
        delivered += 1;
    }
    assert_eq!(delivered, OUTPUT_QUEUE);
}

#[tokio::test]
async fn losing_the_connection_closes_events_and_output() {
    let home = Home::new("drop");
    let _server = fake_daemon(&home, |_, stream| async move { drop(stream) });
    let client = DaemonClient::connect(home.path()).await.unwrap();
    let mut events = client.events().unwrap();
    let mut output = client.subscribe_output(SessionId::new()).unwrap();

    assert!(matches!(
        events.recv().await,
        Err(broadcast::error::RecvError::Closed)
    ));
    assert!(output.recv().await.is_none());
}

#[test]
fn home_prefers_goko_home_then_application_support() {
    let dir = home_dir_from(Some("/tmp/goko".into()), Some("/Users/me".into())).unwrap();
    assert_eq!(dir, PathBuf::from("/tmp/goko"));

    for goko_home in [None, Some("".into())] {
        let dir = home_dir_from(goko_home, Some("/Users/me".into())).unwrap();
        assert_eq!(
            dir,
            PathBuf::from("/Users/me/Library/Application Support/Goko")
        );
    }
    assert!(home_dir_from(None, None).is_err());
}

mod common;

use common::{frame_bytes, TempDir, TestServer};
use protocol::PROTOCOL_VERSION;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

#[tokio::test]
async fn good_hello_gets_hello_ok() {
    let server = TestServer::start();
    let mut client = server.connect().await;

    client.hello(PROTOCOL_VERSION, &server.token).await;

    let reply = client.recv().await.expect("HelloOk");
    assert_eq!(reply["daemon_version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn wrong_token_gets_auth_failed_and_the_socket_closes() {
    let server = TestServer::start();
    let mut client = server.connect().await;

    client.hello(PROTOCOL_VERSION, &"0".repeat(64)).await;

    let reply = client.recv().await.expect("error response");
    assert_eq!(reply["error"]["code"], "auth_failed");
    assert!(client.recv().await.is_none(), "connection should be closed");
}

/// A client that connects and never says hello is told so and dropped,
/// instead of holding its socket until it goes away on its own.
#[tokio::test]
async fn a_connection_without_a_hello_is_closed() {
    let server = TestServer::start();
    let mut client = server.connect().await;

    let started = std::time::Instant::now();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(8), client.recv())
        .await
        .expect("the daemon gave up on the silent connection")
        .expect("error response");
    assert_eq!(reply["error"]["code"], "malformed");
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    assert!(client.recv().await.is_none(), "connection should be closed");
}

#[tokio::test]
async fn wrong_version_gets_version_unsupported_and_the_socket_closes() {
    let server = TestServer::start();
    let mut client = server.connect().await;

    client.hello(PROTOCOL_VERSION + 1, &server.token).await;

    let reply = client.recv().await.expect("error response");
    assert_eq!(reply["error"]["code"], "version_unsupported");
    assert!(client.recv().await.is_none(), "connection should be closed");
}

#[tokio::test]
async fn nothing_is_served_before_the_hello() {
    let server = TestServer::start();
    let mut client = server.connect().await;

    client
        .send(&json!({"type": "system.health", "id": 1}))
        .await;
    let reply = client.recv().await.expect("error response");

    assert_eq!(reply["error"]["code"], "malformed");
    assert!(reply.get("result").is_none());
    assert!(client.recv().await.is_none(), "connection should be closed");
}

#[tokio::test]
async fn a_client_that_leaves_mid_handshake_does_not_disturb_the_server() {
    let server = TestServer::start();
    let mut partial = server.connect().await;
    partial.send_raw(&frame_bytes(0, b"{}")[..3]).await;
    drop(partial);

    let mut client = server.authenticated().await;
    let reply = client
        .request(json!({"type": "system.health", "id": 1}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");
}

#[tokio::test]
async fn a_leftover_socket_file_is_replaced() {
    let home = TempDir::new("stale-socket");
    let socket = home.path().join("daemon.sock");
    drop(UnixListener::bind(&socket).expect("create a stale socket"));
    assert!(socket.exists());

    let server = TestServer::start_in(home);

    server.authenticated().await;
}

#[tokio::test]
async fn socket_file_mode_is_0600() {
    let server = TestServer::start();

    let mode = std::fs::metadata(server.paths.socket())
        .unwrap()
        .permissions()
        .mode();

    assert_eq!(mode & 0o777, 0o600);
}

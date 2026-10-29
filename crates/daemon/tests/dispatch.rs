mod common;

use common::{frame_bytes, TestServer};
use protocol::PROTOCOL_VERSION;
use serde_json::json;

#[tokio::test]
async fn health_reports_ok_and_uptime() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    let reply = client
        .request(json!({"type": "system.health", "id": 7}))
        .await;

    assert_eq!(reply["id"], 7);
    assert_eq!(reply["result"]["status"], "ok");
    assert!(reply["result"]["uptime_seconds"].is_u64());
}

#[tokio::test]
async fn version_reports_daemon_and_protocol_versions() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    let reply = client
        .request(json!({"type": "system.version", "id": 8}))
        .await;

    assert_eq!(reply["id"], 8);
    assert_eq!(reply["result"]["daemon_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(reply["result"]["protocol_version"], PROTOCOL_VERSION);
}

#[tokio::test]
async fn unimplemented_request_is_not_found_and_echoes_its_id() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    let reply = client
        .request(json!({"type": "session.list", "id": 9}))
        .await;

    assert_eq!(reply["id"], 9);
    assert_eq!(reply["error"]["code"], "not_found");
}

#[tokio::test]
async fn malformed_requests_get_an_error_and_the_connection_keeps_working() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    client.send_raw(&frame_bytes(0, b"{not json")).await;
    let reply = client.recv().await.expect("error for invalid JSON");
    assert_eq!(reply["id"], 0);
    assert_eq!(reply["error"]["code"], "malformed");

    let reply = client
        .request(json!({"type": "does.not.exist", "id": 4}))
        .await;
    assert_eq!(reply["id"], 4);
    assert_eq!(reply["error"]["code"], "malformed");

    let reply = client.request(json!({"type": "system.health"})).await;
    assert_eq!(reply["error"]["code"], "malformed");

    let reply = client
        .request(json!({"type": "system.health", "id": 5}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");
}

#[tokio::test]
async fn an_oversized_frame_closes_only_that_connection() {
    let server = TestServer::start();
    let mut offender = server.authenticated().await;
    let mut bystander = server.authenticated().await;

    let too_large = (1024_u32 * 1024 + 1).to_be_bytes();
    offender
        .send_raw(&[too_large[0], too_large[1], too_large[2], too_large[3], 0])
        .await;

    let reply = offender.recv().await.expect("error before close");
    assert_eq!(reply["error"]["code"], "frame_too_large");
    assert!(offender.recv().await.is_none(), "offender should be closed");

    let reply = bystander
        .request(json!({"type": "system.health", "id": 1}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");
}

#[tokio::test]
async fn a_truncated_frame_closes_only_that_connection() {
    let server = TestServer::start();
    let mut offender = server.authenticated().await;
    let mut bystander = server.authenticated().await;

    offender
        .send_raw(&frame_bytes(0, b"{\"type\":\"system.health\",\"id\":1}")[..10])
        .await;
    drop(offender);

    let reply = bystander
        .request(json!({"type": "system.health", "id": 1}))
        .await;
    assert_eq!(reply["result"]["status"], "ok");
}

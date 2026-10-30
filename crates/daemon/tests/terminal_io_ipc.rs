mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use common::{Client, Incoming, TestServer};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn create(argv: &[&str]) -> Value {
    json!({
        "type": "session.create",
        "argv": argv,
        "cwd": std::env::current_dir().unwrap().to_string_lossy(),
        "cols": 80,
        "rows": 24,
        "title": "",
    })
}

fn write_request(id: &Value, text: &str) -> Value {
    json!({"type": "terminal.write", "session_id": id, "data_b64": STANDARD.encode(text)})
}

async fn subscribe(client: &mut Client, id: &Value) {
    let reply = client
        .request(json!({"type": "terminal.subscribe", "session_id": id}))
        .await;
    assert!(reply.get("error").is_none(), "{reply}");
}

/// Read output frames until the text so far contains `needle`.
async fn wait_for_text(client: &mut Client, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    while !text.contains(needle) {
        assert!(
            Instant::now() < deadline,
            "never saw {needle:?}; got {text:?}"
        );
        if let Some(Incoming::Output { bytes, .. }) =
            client.next_within(Duration::from_millis(100)).await
        {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

#[tokio::test]
async fn written_input_reaches_the_process_and_its_echo_comes_back() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    subscribe(&mut client, &id).await;

    let reply = client.request(write_request(&id, "hello\n")).await;

    assert_eq!(reply["result"], json!({}));
    wait_for_text(&mut client, "hello").await;
}

#[tokio::test]
async fn resizing_changes_the_size_the_process_sees() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client
        .request(create(&["sh", "-c", "read ready; stty size"]))
        .await;
    let id = created["result"]["id"].clone();
    subscribe(&mut client, &id).await;

    let resized = client
        .request(json!({"type": "terminal.resize", "session_id": id, "cols": 90, "rows": 25}))
        .await;
    assert_eq!(resized["result"], json!({}));
    client.request(write_request(&id, "go\n")).await;

    wait_for_text(&mut client, "25 90").await;
}

#[tokio::test]
async fn invalid_requests_get_typed_errors() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let unknown = json!("00000000-0000-4000-8000-000000000000");

    let bad_base64 = client
        .request(json!({"type": "terminal.write", "session_id": id, "data_b64": "!!not base64!!"}))
        .await;
    assert_eq!(bad_base64["error"]["code"], "malformed");

    for size in [(0, 0), (1001, 10), (10, 1001)] {
        let reply = client
            .request(json!({"type": "terminal.resize", "session_id": id, "cols": size.0, "rows": size.1}))
            .await;
        assert_eq!(reply["error"]["code"], "invalid_argument", "{size:?}");
    }

    for request in [
        write_request(&unknown, "x"),
        json!({"type": "terminal.resize", "session_id": unknown, "cols": 80, "rows": 24}),
        json!({"type": "terminal.subscribe", "session_id": unknown}),
    ] {
        let reply = client.request(request).await;
        assert_eq!(reply["error"]["code"], "not_found");
    }

    let mut failed_request = create(&["cat"]);
    failed_request["cwd"] = json!("/definitely/not/a/directory");
    let failed = client.request(failed_request).await;
    let reply = client
        .request(json!({"type": "terminal.subscribe", "session_id": failed["result"]["id"]}))
        .await;
    assert_eq!(
        reply["error"]["code"], "invalid_argument",
        "a failed session has no terminal"
    );
}

#[tokio::test]
async fn writing_to_an_exited_session_is_an_invalid_argument_with_a_message() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["sh", "-c", "exit 0"])).await;
    let id = created["result"]["id"].clone();
    for _ in 0..500 {
        let info = client
            .request(json!({"type": "session.get", "session_id": id}))
            .await;
        if info["result"]["state"] == "exited" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let reply = client.request(write_request(&id, "late\n")).await;

    assert_eq!(reply["error"]["code"], "invalid_argument");
    assert!(!reply["error"]["message"].as_str().unwrap().is_empty());
    let resized = client
        .request(json!({"type": "terminal.resize", "session_id": id, "cols": 80, "rows": 24}))
        .await;
    assert_eq!(resized["error"]["code"], "invalid_argument");
}

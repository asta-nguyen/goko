mod common;

use common::{Client, Incoming, TestServer};
use serde_json::{json, Value};
use std::time::Duration;

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

async fn wait_for_state(client: &mut Client, session_id: &Value, state: &str) -> Value {
    for _ in 0..500 {
        let reply = client
            .request(json!({"type": "session.get", "session_id": session_id}))
            .await;
        if reply["result"]["state"] == state {
            return reply["result"].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("session never reached {state}");
}

#[tokio::test]
async fn create_list_get_and_stop_work_over_the_socket() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    let created = client.request(create(&["cat"])).await;
    let session = &created["result"];
    assert_eq!(session["state"], "running");
    assert_eq!(session["title"], "cat");

    let listed = client.request(json!({"type": "session.list"})).await;
    assert_eq!(listed["result"]["sessions"], json!([session]));
    let fetched = client
        .request(json!({"type": "session.get", "session_id": session["id"]}))
        .await;
    assert_eq!(fetched["result"], *session);

    let stopped = client
        .request(json!({"type": "session.stop", "session_id": session["id"], "force": false}))
        .await;
    assert_eq!(stopped["result"]["state"], "stopping");
    let exited = wait_for_state(&mut client, &session["id"], "exited").await;
    assert_eq!(exited["exit_code"], Value::Null, "killed by SIGTERM");

    let again = client
        .request(json!({"type": "session.stop", "session_id": session["id"], "force": true}))
        .await;
    assert_eq!(again["result"]["state"], "exited", "stopping twice is fine");
}

#[tokio::test]
async fn only_a_subscribed_client_receives_session_events() {
    let server = TestServer::start();
    let mut subscribed = server.authenticated().await;
    let mut bystander = server.authenticated().await;
    let reply = subscribed
        .request(json!({"type": "events.subscribe"}))
        .await;
    assert_eq!(reply["result"], json!({}));

    let created = subscribed.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    subscribed
        .request(json!({"type": "session.stop", "session_id": id, "force": false}))
        .await;

    let mut states = Vec::new();
    let mut exited_event = None;
    while exited_event.is_none() {
        let Some(Incoming::Control(event)) = subscribed.next().await else {
            panic!("the event stream ended early");
        };
        match event["type"].as_str() {
            Some("session.updated") => {
                assert_eq!(event["session"]["id"], id);
                states.push(event["session"]["state"].as_str().unwrap().to_owned());
            }
            Some("session.exited") => exited_event = Some(event),
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert_eq!(states, ["starting", "running", "stopping", "exited"]);
    let exited_event = exited_event.unwrap();
    assert_eq!(exited_event["session_id"], id);
    assert_eq!(exited_event["exit_code"], Value::Null);

    assert_eq!(
        bystander.next_within(Duration::from_millis(300)).await,
        None,
        "a client that never subscribed receives no events"
    );
}

#[tokio::test]
async fn bad_input_gets_typed_errors() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let unknown = "00000000-0000-4000-8000-000000000000";

    for request in [
        json!({"type": "session.stop", "session_id": unknown, "force": false}),
        json!({"type": "session.get", "session_id": unknown}),
    ] {
        let reply = client.request(request).await;
        assert_eq!(reply["error"]["code"], "not_found");
    }

    let mut no_command = create(&[]);
    no_command["argv"] = json!([]);
    let mut no_columns = create(&["cat"]);
    no_columns["cols"] = json!(0);
    for request in [no_command, no_columns] {
        let reply = client.request(request).await;
        assert_eq!(reply["error"]["code"], "invalid_argument");
    }
    let listed = client.request(json!({"type": "session.list"})).await;
    assert_eq!(listed["result"]["sessions"], json!([]));
}

#[tokio::test]
async fn a_bad_cwd_returns_a_failed_session_not_an_error() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let mut request = create(&["cat"]);
    request["cwd"] = json!("/definitely/not/a/directory");

    let reply = client.request(request).await;

    assert!(reply.get("error").is_none());
    assert_eq!(reply["result"]["state"], "failed");
    assert!(reply["result"]["error"]
        .as_str()
        .unwrap()
        .contains("working directory"));
}

#[tokio::test]
async fn sessions_are_listed_in_creation_order() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let first = client.request(create(&["cat"])).await;
    let second = client.request(create(&["sleep", "30"])).await;

    let listed = client.request(json!({"type": "session.list"})).await;

    let ids: Vec<&Value> = listed["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| &session["id"])
        .collect();
    assert_eq!(ids, [&first["result"]["id"], &second["result"]["id"]]);
    server.sessions.shutdown().await;
}

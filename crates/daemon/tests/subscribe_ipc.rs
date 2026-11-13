mod common;

use common::{frame_bytes, read_incoming, Client, Incoming, TestServer};
use protocol::session::{SessionId, SessionState};
use protocol::PROTOCOL_VERSION;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

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

fn session_id(value: &Value) -> SessionId {
    serde_json::from_value(value.clone()).expect("a session id")
}

/// Subscribe and return `(replay_from, next_seq)`.
async fn subscribe(client: &mut Client, id: &Value) -> (u64, u64) {
    let reply = client
        .request(json!({"type": "terminal.subscribe", "session_id": id}))
        .await;
    (
        reply["result"]["replay_from"]
            .as_u64()
            .expect("replay_from"),
        reply["result"]["next_seq"].as_u64().expect("next_seq"),
    )
}

/// Subscribe with `flow: true` and return `(replay_from, next_seq)`.
async fn subscribe_flow(client: &mut Client, id: &Value) -> (u64, u64) {
    let reply = client
        .request(json!({"type": "terminal.subscribe", "session_id": id, "flow": true}))
        .await;
    (
        reply["result"]["replay_from"]
            .as_u64()
            .expect("replay_from"),
        reply["result"]["next_seq"].as_u64().expect("next_seq"),
    )
}

/// Send `terminal.ack` for `upto_seq` and return the response.
async fn ack(client: &mut Client, id: &Value, upto_seq: u64) -> Value {
    client
        .request(json!({
            "type": "terminal.ack",
            "session_id": id,
            "upto_seq": upto_seq,
        }))
        .await
}

/// Output frames until none arrives for `quiet`. The first frame must start at
/// `first_seq` and each must continue the previous one.
async fn collect_output(client: &mut Client, first_seq: u64, quiet: Duration) -> Vec<u8> {
    let mut stream = Vec::new();
    let mut next_seq = first_seq;
    while let Some(frame) = client.next_within(quiet).await {
        if let Incoming::Output { seq, bytes, .. } = frame {
            assert_eq!(seq, next_seq, "output frame does not continue the stream");
            next_seq += bytes.len() as u64;
            stream.extend_from_slice(&bytes);
        }
    }
    stream
}

#[tokio::test]
async fn output_frames_are_contiguous_and_match_the_buffer() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let script = "sleep 0.1; i=0; while [ $i -lt 120 ]; do echo line$i; i=$((i+1)); done";
    let created = client.request(create(&["sh", "-c", script])).await;
    let id = &created["result"]["id"];
    let pty = server.sessions.pty(session_id(id)).unwrap();

    let (replay_from, _) = subscribe(&mut client, id).await;
    assert_eq!(replay_from, 0);
    pty.wait_exit().await.unwrap();
    let stream = collect_output(&mut client, replay_from, Duration::from_millis(400)).await;

    let (_, retained, _) = pty.buffer();
    assert!(String::from_utf8_lossy(&retained).contains("line119"));
    assert_eq!(stream, retained, "the client missed or repeated bytes");
}

#[tokio::test]
async fn a_second_client_subscribing_mid_stream_gets_no_gap_or_duplicate() {
    let server = TestServer::start();
    let mut first = server.authenticated().await;
    let mut second = server.authenticated().await;
    let created = first.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    let (first_from, _) = subscribe(&mut first, &id).await;

    let feeder = {
        let pty = Arc::clone(&pty);
        tokio::spawn(async move {
            for _ in 0..100 {
                pty.write(b"x\n").unwrap();
                tokio::time::sleep(Duration::from_millis(4)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (second_from, second_next) = subscribe(&mut second, &id).await;
    assert!(second_next > 0, "the second client joined mid-stream");
    feeder.await.unwrap();
    pty.write(&[0x04]).unwrap();
    pty.wait_exit().await.unwrap();

    let quiet = Duration::from_millis(400);
    let first_stream = collect_output(&mut first, first_from, quiet).await;
    let second_stream = collect_output(&mut second, second_from, quiet).await;
    let (replay_from, retained, _) = pty.buffer();
    assert_eq!((first_from, second_from, replay_from), (0, 0, 0));
    assert_eq!(first_stream, retained);
    assert_eq!(second_stream, retained);
}

#[tokio::test]
async fn no_frames_follow_an_unsubscribe() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    subscribe(&mut client, &id).await;
    pty.write(b"a\n").unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reply = client
        .request(json!({"type": "terminal.unsubscribe", "session_id": id}))
        .await;
    assert_eq!(reply["result"], json!({}));
    pty.write(b"b\n").unwrap();

    let mut seen = Vec::new();
    while let Some(frame) = client.next_within(Duration::from_millis(300)).await {
        if let Incoming::Output { bytes, .. } = frame {
            seen.extend(bytes);
        }
    }
    assert!(
        seen.contains(&b'a'),
        "output before the unsubscribe is delivered"
    );
    assert!(!seen.contains(&b'b'), "no output after the unsubscribe");
}

#[tokio::test]
async fn a_dropped_connection_leaves_the_session_running() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    subscribe(&mut client, &id).await;
    assert_eq!(pty.subscriber_count(), 1);

    drop(client);
    // A removed subscriber leaves the registry on the next output.
    for _ in 0..200 {
        pty.write(b"x\n").unwrap();
        if pty.subscriber_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(pty.subscriber_count(), 0);
    let info = server.sessions.get(session_id(&id)).unwrap();
    assert_eq!(info.state, protocol::session::SessionState::Running);
}

#[tokio::test]
async fn a_stalled_client_gets_lagged_while_another_client_is_unaffected() {
    let server = TestServer::start();
    let mut healthy = server.authenticated().await;
    let created = healthy.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    let (healthy_from, _) = subscribe(&mut healthy, &id).await;

    // A client that subscribes and then never reads.
    let mut stalled = UnixStream::connect(server.paths.socket()).await.unwrap();
    let hello = json!({"protocol_version": PROTOCOL_VERSION, "token": server.token});
    stalled
        .write_all(&frame_bytes(0, &serde_json::to_vec(&hello).unwrap()))
        .await
        .unwrap();
    assert!(read_incoming(&mut stalled).await.is_some(), "HelloOk");
    let subscribe_request = json!({"type": "terminal.subscribe", "id": 1, "session_id": id});
    stalled
        .write_all(&frame_bytes(
            0,
            &serde_json::to_vec(&subscribe_request).unwrap(),
        ))
        .await
        .unwrap();
    assert!(read_incoming(&mut stalled).await.is_some(), "the response");

    // Enough separate chunks to overflow the stalled client's socket, writer queue, and
    // 256-chunk subscription queue.
    for _ in 0..1500 {
        pty.write(b"x\n").unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    pty.write(&[0x04]).unwrap();
    pty.wait_exit().await.unwrap();

    let mut frames_before_lag = 0;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), read_incoming(&mut stalled))
            .await
            .expect("the stalled client should be told it lagged")
            .expect("the connection stays open");
        match frame {
            Incoming::Output { .. } => frames_before_lag += 1,
            Incoming::Control(event) => {
                assert_eq!(event["type"], "terminal.lagged");
                assert_eq!(event["session_id"], id);
                break;
            }
        }
    }
    assert!(frames_before_lag > 0);

    let stream = collect_output(&mut healthy, healthy_from, Duration::from_millis(400)).await;
    let (_, retained, _) = pty.buffer();
    assert_eq!(stream, retained, "the healthy client got every byte");
    let info = server.sessions.get(session_id(&id)).unwrap();
    assert_eq!(info.exit_code, Some(0), "the session exited normally");
}

#[tokio::test]
async fn subscribing_to_an_exited_session_yields_the_snapshot_then_nothing() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["sh", "-c", "echo done"])).await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    pty.wait_exit().await.unwrap();
    // Subscribing until the reader has closed makes the stream end right after the snapshot.
    for _ in 0..200 {
        let (_, mut receiver) = pty.subscribe();
        if receiver.recv().await == Some(daemon::pty::StreamItem::Closed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let (replay_from, next_seq) = subscribe(&mut client, &id).await;

    assert_eq!((replay_from, next_seq), (0, 6));
    let frame = client.next().await.expect("the snapshot frame");
    let Incoming::Output { seq, bytes, .. } = frame else {
        panic!("expected an output frame");
    };
    assert_eq!((seq, bytes.as_slice()), (0, b"done\r\n".as_slice()));
    assert_eq!(client.next_within(Duration::from_millis(300)).await, None);
}

#[tokio::test]
async fn deleting_a_finished_session_stops_existing_subscriptions_and_rejects_new_ones() {
    let server = TestServer::start();
    let mut first = server.authenticated().await;
    let mut second = server.authenticated().await;
    let created = first.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let session_id = session_id(&id);
    subscribe(&mut first, &id).await;
    subscribe(&mut second, &id).await;

    server.sessions.stop(session_id, false).unwrap();
    for _ in 0..500 {
        if server.sessions.get(session_id).unwrap().state == SessionState::Exited {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        server.sessions.get(session_id).unwrap().state,
        SessionState::Exited
    );

    let deleted = first
        .request(json!({"type": "session.delete", "session_id": id}))
        .await;
    assert_eq!(deleted["result"], json!({}));
    assert!(matches!(
        server.sessions.pty(session_id),
        Err(daemon::session::SessionError::NotFound(_))
    ));
    assert_eq!(first.next_within(Duration::from_millis(100)).await, None);
    assert_eq!(second.next_within(Duration::from_millis(100)).await, None);

    let rejected = first
        .request(json!({"type": "terminal.subscribe", "session_id": id}))
        .await;
    assert_eq!(rejected["error"]["code"], "not_found");
}

#[tokio::test]
async fn the_response_arrives_before_any_output_frame() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client
        .request(create(&["sh", "-c", "echo before; sleep 30"]))
        .await;
    let id = created["result"]["id"].clone();
    let pty = server.sessions.pty(session_id(&id)).unwrap();
    for _ in 0..500 {
        if !pty.buffer().1.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    client
        .send(&json!({"type": "terminal.subscribe", "id": 7, "session_id": id}))
        .await;

    let Some(Incoming::Control(response)) = client.next().await else {
        panic!("the first frame must be the response, not output");
    };
    assert_eq!(response["id"], 7);
    let next_seq = response["result"]["next_seq"].as_u64().unwrap();
    assert!(
        next_seq > 0,
        "the retained output is announced by the response"
    );
    let Some(Incoming::Output { seq, .. }) = client.next().await else {
        panic!("the retained output follows the response");
    };
    assert_eq!(seq, response["result"]["replay_from"].as_u64().unwrap());
}

/// The flow window and the one in-flight read the reader may overshoot by.
const WINDOW: u64 = 256 * 1024;
const READ_OVERRUN: u64 = 16 * 1024;
const MEBIBYTE: usize = 1024 * 1024;

/// Poll `condition` until it holds or `limit` passes, then panic naming `what`.
async fn wait_until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Wait until the session's published offset stops moving for 200 ms: whatever
/// its process was still writing has been published.
async fn wait_for_quiet(pty: &daemon::pty::PtyProcess) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut last, mut still) = (pty.output_info().0, 0);
    while still < 4 {
        assert!(Instant::now() < deadline, "the output never settled");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = pty.output_info().0;
        if now == last {
            still += 1;
        } else {
            (last, still) = (now, 0);
        }
    }
}

/// The `PtyProcess` behind a created session id.
fn server_pty(server: &TestServer, id: &Value) -> Arc<daemon::pty::PtyProcess> {
    server
        .sessions
        .pty(session_id(id))
        .expect("session process")
}

/// Spawn a session whose producer is done, leaving a full 1 MiB replay buffer.
async fn finished_producer(
    server: &TestServer,
    client: &mut Client,
) -> (Value, Arc<daemon::pty::PtyProcess>) {
    let created = client
        .request(create(&["sh", "-c", "yes | head -c 2097152"]))
        .await;
    let id = created["result"]["id"].clone();
    let pty = server_pty(server, &id);
    pty.wait_exit().await.expect("the producer exits");
    wait_until("the replay buffer to fill", Duration::from_secs(10), || {
        pty.buffer().1.len() == MEBIBYTE
    })
    .await;
    // The process has exited, but the reader may still be taking its last
    // output from the PTY; a subscribe before that would see it arrive live.
    wait_for_quiet(&pty).await;
    (id, pty)
}

#[tokio::test]
async fn a_flow_subscribe_stops_at_the_window_until_an_ack() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["yes"])).await;
    let id = created["result"]["id"].clone();
    let pty = server_pty(&server, &id);
    wait_until("enough retained output", Duration::from_secs(10), || {
        pty.buffer().1.len() >= 600 * 1024
    })
    .await;

    let (replay_from, _) = subscribe_flow(&mut client, &id).await;
    let first = collect_output(&mut client, replay_from, Duration::from_millis(700)).await;
    assert_eq!(
        first.len() as u64,
        WINDOW,
        "a flow stream stops after one window without acks"
    );

    let reply = ack(&mut client, &id, replay_from + first.len() as u64).await;
    assert_eq!(reply["result"], json!({}));
    let second = collect_output(
        &mut client,
        replay_from + first.len() as u64,
        Duration::from_millis(700),
    )
    .await;
    assert_eq!(
        second.len() as u64,
        WINDOW,
        "the ack releases exactly one more window of output"
    );
}

#[tokio::test]
async fn an_ack_without_a_flow_subscribe_is_accepted_and_delivers_normally() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client.request(create(&["cat"])).await;
    let id = created["result"]["id"].clone();
    let pty = server_pty(&server, &id);
    let (replay_from, _) = subscribe(&mut client, &id).await;

    let reply = ack(&mut client, &id, 8192).await;
    assert_eq!(reply["result"], json!({}));

    // The subscription was not flow-controlled, so the ack gates nothing.
    pty.write(b"ping\n").expect("write to cat");
    let stream = collect_output(&mut client, replay_from, Duration::from_millis(500)).await;
    assert!(
        String::from_utf8_lossy(&stream).contains("ping"),
        "the non-flow stream kept delivering"
    );
}

#[tokio::test]
async fn an_ack_for_an_unknown_session_is_not_found() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;

    let reply = client
        .request(json!({
            "type": "terminal.ack",
            "session_id": "00000000-0000-4000-8000-000000000000",
            "upto_seq": 1,
        }))
        .await;

    assert_eq!(reply["error"]["code"], "not_found");
}

#[tokio::test]
async fn a_flow_subscribe_paces_replay_by_the_same_window() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let (id, pty) = finished_producer(&server, &mut client).await;

    let (replay_from, _) = subscribe_flow(&mut client, &id).await;
    let mut received = collect_output(&mut client, replay_from, Duration::from_millis(600)).await;
    assert_eq!(
        received.len() as u64,
        WINDOW,
        "replay stops after one window without acks"
    );

    let mut upto = replay_from + received.len() as u64;
    while received.len() < MEBIBYTE {
        let reply = ack(&mut client, &id, upto).await;
        assert_eq!(reply["result"], json!({}));
        let more = collect_output(&mut client, upto, Duration::from_millis(600)).await;
        assert!(!more.is_empty(), "the ack did not release more replay");
        received.extend_from_slice(&more);
        upto += more.len() as u64;
    }

    let (_, retained, _) = pty.buffer();
    assert_eq!(
        received, retained,
        "the paced replay missed or repeated bytes"
    );
}

#[tokio::test]
async fn a_flow_resubscribe_after_acking_next_seq_gets_the_whole_snapshot() {
    let server = TestServer::start();
    let mut first = server.authenticated().await;
    let (id, pty) = finished_producer(&server, &mut first).await;

    let (replay_from, next_seq) = subscribe_flow(&mut first, &id).await;
    let reply = ack(&mut first, &id, next_seq).await;
    assert_eq!(reply["result"], json!({}));

    // A new mount acknowledges nothing, yet the acknowledged offset already
    // covers every snapshot piece, so nothing waits.
    let mut second = server.authenticated().await;
    let (replay_from2, _) = subscribe_flow(&mut second, &id).await;
    assert_eq!(replay_from2, replay_from);
    let stream = collect_output(&mut second, replay_from2, Duration::from_millis(600)).await;

    let (_, retained, _) = pty.buffer();
    assert_eq!(stream, retained, "the resubscribe withheld paced bytes");
}

#[tokio::test]
async fn a_flow_resubscribe_after_a_partial_ack_paces_from_that_offset() {
    let server = TestServer::start();
    let mut first = server.authenticated().await;
    let (id, _) = finished_producer(&server, &mut first).await;

    let (replay_from, _) = subscribe_flow(&mut first, &id).await;
    let piece = 64 * 1024_u64;
    let reply = ack(&mut first, &id, replay_from + piece).await;
    assert_eq!(reply["result"], json!({}));

    let mut second = server.authenticated().await;
    let (replay_from2, _) = subscribe_flow(&mut second, &id).await;
    assert_eq!(replay_from2, replay_from);
    let stream = collect_output(&mut second, replay_from2, Duration::from_millis(600)).await;
    assert_eq!(
        stream.len() as u64,
        piece + WINDOW,
        "the second snapshot delivers up to the acknowledged offset plus one window"
    );

    // The stream waits on credit, not on a closed producer: one more ack
    // releases one more window.
    let upto = replay_from2 + stream.len() as u64;
    let reply = ack(&mut second, &id, upto).await;
    assert_eq!(reply["result"], json!({}));
    let more = collect_output(&mut second, upto, Duration::from_millis(600)).await;
    assert_eq!(more.len() as u64, WINDOW);
}

#[tokio::test]
async fn a_remount_with_a_far_ahead_offset_gets_one_burst_plus_a_live_window() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client
        .request(create(&["sh", "-c", "stty -echo -icanon; cat"]))
        .await;
    let id = created["result"]["id"].clone();
    let pty = server_pty(&server, &id);

    // Wait for cat to own the pty settings, then drive over a megabyte through
    // it with no subscriber attached.
    pty.write(b"probe\n").expect("write to cat");
    wait_until("cat to take over the pty", Duration::from_secs(5), || {
        pty.buffer().2 > 0
    })
    .await;
    let feed = vec![b'x'; 1_400_000];
    tokio::task::spawn_blocking({
        let pty = Arc::clone(&pty);
        move || pty.write(&feed)
    })
    .await
    .expect("the feed task")
    .expect("write to cat");
    wait_until("the replay buffer to roll", Duration::from_secs(10), || {
        pty.buffer().1.len() == MEBIBYTE
    })
    .await;
    // `cat` may still be writing the end of the feed; the burst below must be
    // the retained snapshot alone, not that live tail.
    wait_for_quiet(&pty).await;
    let published = pty.buffer().2;

    // The previous mount acknowledged everything it received before going away.
    let reply = ack(&mut client, &id, published).await;
    assert_eq!(reply["result"], json!({}));

    // The remount's acknowledged offset covers the whole retained snapshot, so
    // the megabyte arrives as one burst without pacing waits.
    let (replay_from, next_seq) = subscribe_flow(&mut client, &id).await;
    let burst = collect_output(&mut client, replay_from, Duration::from_millis(700)).await;
    assert_eq!(
        burst.len() as u64,
        next_seq - replay_from,
        "the remount burst is not the whole retained snapshot"
    );
    assert!(burst.len() >= MEBIBYTE - 64 * 1024);
    let mut upto = next_seq;

    // Live output continues into one window past the acknowledged offset, then
    // the gate stops it. The feed blocks behind the gated reader.
    let live = vec![b'y'; 400 * 1024];
    let feeder = tokio::task::spawn_blocking({
        let pty = Arc::clone(&pty);
        move || pty.write(&live)
    });
    let window = collect_output(&mut client, upto, Duration::from_millis(800)).await;
    upto += window.len() as u64;
    assert!(
        window.len() as u64 <= WINDOW + READ_OVERRUN,
        "live output past the acknowledged offset exceeds one window plus a read"
    );
    assert!(upto > published, "no live output followed the burst");
    assert_eq!(
        pty.buffer().2,
        upto,
        "the reader published past the gated window"
    );

    // An ack reopens the window and the rest of the feed arrives.
    let reply = ack(&mut client, &id, upto).await;
    assert_eq!(reply["result"], json!({}));
    feeder.await.expect("the feed task").expect("write to cat");
    let rest = collect_output(&mut client, upto, Duration::from_millis(800)).await;
    assert!(!rest.is_empty(), "the ack did not resume live output");
}

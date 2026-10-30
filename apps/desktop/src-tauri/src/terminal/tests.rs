use super::*;
use crate::test_support::{fake_daemon, read_request, reply, send_json, send_output, Home};
use protocol::session::SessionState;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::timeout;

type Frames = UnboundedReceiver<Vec<u8>>;

/// A sink the test can read; dropping the receiver closes it, like a closed webview channel.
fn sink() -> (Sink, Frames) {
    let (tx, rx) = unbounded_channel();
    (Box::new(move |frame| tx.send(frame).is_ok()), rx)
}

/// Decoded `(seq, bytes)` messages until `total` payload bytes have arrived.
async fn collect(frames: &mut Frames, total: usize) -> Vec<(u64, Vec<u8>)> {
    let mut got = Vec::new();
    let mut have = 0;
    timeout(Duration::from_secs(10), async {
        while have < total {
            let frame = frames.recv().await.expect("the stream ended early");
            let seq = u64::from_be_bytes(frame[..8].try_into().unwrap());
            have += frame.len() - 8;
            got.push((seq, frame[8..].to_vec()));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("only {have} of {total} bytes arrived"));
    got
}

/// The bytes of `chunks`, asserting they are contiguous from `start` with nothing repeated.
fn joined(chunks: &[(u64, Vec<u8>)], start: u64) -> Vec<u8> {
    let mut next = start;
    let mut bytes = Vec::new();
    for (seq, data) in chunks {
        assert_eq!(*seq, next, "chunks must be contiguous: {chunks:?}");
        next += data.len() as u64;
        bytes.extend_from_slice(data);
    }
    bytes
}

async fn cat(bridge: &Bridge) -> SessionId {
    let session = bridge
        .session_create("cat", "/tmp".into(), 80, 24, None)
        .await
        .unwrap();
    assert_ne!(session.state, SessionState::Failed);
    session.id
}

/// The terminal echoes each typed line and `cat` prints it back.
fn echoed(line: &str) -> Vec<u8> {
    format!("{line}\r\n{line}\r\n").into_bytes()
}

#[tokio::test]
async fn a_running_session_delivers_replay_then_live_bytes_contiguously() {
    let home = Home::new("term-live");
    let bridge = Bridge::new(home.path().to_owned());
    let id = cat(&bridge).await;
    bridge.terminal_write(id, "one\n").await.unwrap();
    // Let the output exist before subscribing, so it is replayed rather than live.
    let (first, mut seen) = sink();
    bridge.terminal_subscribe(id, first).await.unwrap();
    collect(&mut seen, echoed("one").len()).await;
    bridge.terminal_unsubscribe(id);

    let (second, mut frames) = sink();
    bridge.terminal_subscribe(id, second).await.unwrap();
    bridge.terminal_write(id, "two\n").await.unwrap();

    let want = [echoed("one"), echoed("two")].concat();
    let chunks = collect(&mut frames, want.len()).await;
    assert_eq!(joined(&chunks, 0), want);
}

#[tokio::test]
async fn unsubscribe_then_subscribe_in_quick_succession_keeps_exactly_one_live_stream() {
    let home = Home::new("term-order");
    let bridge = Bridge::new(home.path().to_owned());
    let id = cat(&bridge).await;
    let (first, first_frames) = sink();
    bridge.terminal_subscribe(id, first).await.unwrap();

    // The webview unsubscribes the old view without waiting, then subscribes the new one.
    let mut sinks = Vec::new();
    for _ in 0..5 {
        bridge.terminal_unsubscribe(id);
        let (next, frames) = sink();
        bridge.terminal_subscribe(id, next).await.unwrap();
        sinks.push(frames);
    }
    let mut live = sinks.pop().unwrap();
    // Give any stop that was handled late time to land before the output is produced.
    tokio::time::sleep(Duration::from_millis(200)).await;
    bridge.terminal_write(id, "go\n").await.unwrap();

    // The surviving stream delivers the new output once, from the start, without repeats.
    let chunks = collect(&mut live, echoed("go").len()).await;
    assert_eq!(joined(&chunks, 0), echoed("go"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(live.try_recv().is_err(), "no byte arrived twice");
    // Every replaced stream stopped for good.
    for mut old in sinks.into_iter().chain([first_frames]) {
        assert!(old.try_recv().is_err(), "a replaced stream still delivered");
    }
}

#[tokio::test]
async fn a_closed_sink_makes_the_bridge_unsubscribe_from_the_daemon() {
    let home = Home::new("term-close");
    let bridge = Bridge::new(home.path().to_owned());
    let id = cat(&bridge).await;
    let (stream, mut frames) = sink();
    bridge.terminal_subscribe(id, stream).await.unwrap();
    bridge.terminal_write(id, "a\n").await.unwrap();
    collect(&mut frames, echoed("a").len()).await;

    drop(frames);
    bridge.terminal_write(id, "b\n").await.unwrap(); // the next frame finds the sink closed
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Listen on the shared connection ourselves: a stream the daemon still sent would show.
    let client = bridge.client().await.unwrap();
    let mut probe = client.subscribe_output(id).unwrap();
    bridge.terminal_write(id, "c\n").await.unwrap();
    let quiet = timeout(Duration::from_millis(500), probe.recv()).await;
    assert!(quiet.is_err(), "the daemon kept streaming: {quiet:?}");
}

#[tokio::test]
async fn subscribing_to_an_unknown_session_fails_with_not_found() {
    let home = Home::new("term-none");
    let bridge = Bridge::new(home.path().to_owned());

    let (stream, _frames) = sink();
    let error = bridge
        .terminal_subscribe(SessionId::new(), stream)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        Code::Daemon(protocol::message::ErrorCode::NotFound)
    );
}

/// A stand-in daemon that streams the bytes `0123456789ABCDEF` of `session`, and counts
/// how often it was asked to (re)subscribe.
fn fake_stream(
    home: &Home,
    session: SessionId,
    subscribes: Arc<AtomicUsize>,
    script: fn(usize, usize) -> Script,
) -> tokio::task::JoinHandle<()> {
    fake_daemon(home, move |connection, mut stream| {
        let subscribes = Arc::clone(&subscribes);
        async move {
            while let Some(request) = read_request(&mut stream).await {
                if request["type"] != "terminal.subscribe" {
                    reply(&mut stream, &request, json!({})).await;
                    continue;
                }
                let nth = subscribes.fetch_add(1, Ordering::SeqCst);
                let Script {
                    replay_from,
                    chunks,
                    then,
                } = script(connection, nth);
                reply(
                    &mut stream,
                    &request,
                    json!({"replay_from": replay_from, "next_seq": 0}),
                )
                .await;
                for (seq, data) in chunks {
                    send_output(&mut stream, session, seq, data).await;
                }
                match then {
                    Then::Lag => {
                        send_json(
                            &mut stream,
                            json!({"type": "terminal.lagged", "session_id": session}),
                        )
                        .await
                    }
                    Then::Hangup => return,
                    Then::Stay => {}
                }
            }
        }
    })
}

struct Script {
    replay_from: u64,
    chunks: Vec<(u64, &'static [u8])>,
    then: Then,
}

enum Then {
    Lag,
    Hangup,
    Stay,
}

#[tokio::test]
async fn lagged_resubscribes_exactly_once_and_trims_what_was_already_delivered() {
    let home = Home::new("term-lag");
    let session = SessionId::new();
    let subscribes = Arc::new(AtomicUsize::new(0));
    let _daemon = fake_stream(
        &home,
        session,
        Arc::clone(&subscribes),
        |_, nth| match nth {
            0 => Script {
                replay_from: 0,
                chunks: vec![(0, b"01234567")],
                then: Then::Lag,
            },
            // The replay starts before what the webview already has, and carries on live.
            _ => Script {
                replay_from: 4,
                chunks: vec![(4, b"456789AB"), (12, b"CDEF")],
                then: Then::Stay,
            },
        },
    );
    let bridge = Bridge::new(home.path().to_owned());
    let (stream, mut frames) = sink();

    bridge.terminal_subscribe(session, stream).await.unwrap();

    let chunks = collect(&mut frames, 16).await;
    assert_eq!(joined(&chunks, 0), b"0123456789ABCDEF");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        subscribes.load(Ordering::SeqCst),
        2,
        "one subscribe, one re-subscribe"
    );
    assert!(frames.try_recv().is_err());
}

#[tokio::test]
async fn a_lost_connection_resumes_on_a_new_one_without_duplicates() {
    let home = Home::new("term-redo");
    let session = SessionId::new();
    let subscribes = Arc::new(AtomicUsize::new(0));
    let _daemon = fake_stream(
        &home,
        session,
        Arc::clone(&subscribes),
        |_, nth| match nth {
            0 => Script {
                replay_from: 0,
                chunks: vec![(0, b"01234567")],
                then: Then::Hangup,
            },
            _ => Script {
                replay_from: 4,
                chunks: vec![(4, b"456789AB"), (12, b"CDEF")],
                then: Then::Stay,
            },
        },
    );
    let bridge = Bridge::new(home.path().to_owned());
    let (stream, mut frames) = sink();

    bridge.terminal_subscribe(session, stream).await.unwrap();

    let chunks = collect(&mut frames, 16).await;
    assert_eq!(joined(&chunks, 0), b"0123456789ABCDEF");
    assert_eq!(subscribes.load(Ordering::SeqCst), 2);
    assert!(
        home.spawns().is_empty(),
        "the stand-in daemon was reused, not replaced"
    );
}

#[tokio::test]
async fn bytes_the_daemon_no_longer_holds_show_as_a_jump_in_seq() {
    let home = Home::new("term-gap");
    let session = SessionId::new();
    let subscribes = Arc::new(AtomicUsize::new(0));
    let _daemon = fake_stream(
        &home,
        session,
        Arc::clone(&subscribes),
        |_, nth| match nth {
            0 => Script {
                replay_from: 0,
                chunks: vec![(0, b"0123")],
                then: Then::Lag,
            },
            // The ring buffer moved on: bytes 4..8 are gone.
            _ => Script {
                replay_from: 8,
                chunks: vec![(8, b"89AB")],
                then: Then::Stay,
            },
        },
    );
    let bridge = Bridge::new(home.path().to_owned());
    let (stream, mut frames) = sink();

    bridge.terminal_subscribe(session, stream).await.unwrap();

    let chunks = collect(&mut frames, 8).await;
    assert_eq!(chunks, [(0, b"0123".to_vec()), (8, b"89AB".to_vec())]);
}

#[test]
fn fresh_bytes_only_are_framed_with_the_seq_of_their_first_byte() {
    let chunk = |seq, data: &[u8]| Output {
        seq,
        data: data.to_vec(),
    };
    let framed = |seq: u64, data: &[u8]| [&seq.to_be_bytes()[..], data].concat();
    let mut delivered = None;

    assert_eq!(
        fresh(&mut delivered, chunk(0, b"abcd")),
        Some(framed(0, b"abcd"))
    );
    // A replay that starts earlier: only the unseen tail is kept.
    assert_eq!(
        fresh(&mut delivered, chunk(2, b"cdef")),
        Some(framed(4, b"ef"))
    );
    // Fully seen, and empty: nothing to send.
    assert_eq!(fresh(&mut delivered, chunk(1, b"bcd")), None);
    assert_eq!(fresh(&mut delivered, chunk(6, b"")), None);
    // A gap is passed through whole.
    assert_eq!(
        fresh(&mut delivered, chunk(10, b"xy")),
        Some(framed(10, b"xy"))
    );
    assert_eq!(delivered, Some(12));
}

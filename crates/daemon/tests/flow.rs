use daemon::pty::{PtyProcess, SpawnSpec, StreamItem};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

/// The flow window and the one read the reader may overshoot by.
const WINDOW: u64 = 256 * 1024;
const READ_CHUNK: u64 = 16 * 1024;

fn spec(argv: &[&str]) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir().expect("current directory"),
        cols: 80,
        rows: 24,
    }
}

/// Poll `condition` until it holds or `limit` passes, then panic naming `what`.
async fn wait_until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Consume the stream, asserting each chunk continues `first_seq`, and return
/// the end offset once the sender closes (after `Lagged`, `Closed`, or a
/// removal). Keeps the subscriber queue empty so only the ack gate matters.
async fn collect_after(first_seq: u64, mut receiver: Receiver<StreamItem>) -> u64 {
    let mut next_seq = first_seq;
    while let Some(item) = receiver.recv().await {
        match item {
            StreamItem::Chunk { seq, bytes } => {
                assert_eq!(seq, next_seq, "stream gap or overlap");
                next_seq += bytes.len() as u64;
            }
            StreamItem::Lagged | StreamItem::Closed => break,
        }
    }
    next_seq
}

async fn kill_and_reap(process: &PtyProcess) {
    process.kill().expect("kill the producer");
    process.wait_exit().await.expect("reap the producer");
}

/// (a) A flow subscriber that never acks stops the reader once unacknowledged
/// output reaches the window, plus at most one in-flight read.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_subscriber_without_acks_stops_the_reader_at_the_window() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (snapshot, receiver) = process.subscribe_flow();
    let acknowledged = *process.acknowledged_offset().borrow();
    assert_eq!(acknowledged, snapshot.replay_from);
    let collector = tokio::spawn(collect_after(snapshot.next_seq, receiver));

    wait_until("the window to fill", Duration::from_secs(10), || {
        process.buffer().2 >= acknowledged + WINDOW
    })
    .await;
    let stalled = process.buffer().2;
    assert!(
        stalled <= acknowledged + WINDOW + READ_CHUNK,
        "published {stalled} past acknowledged {acknowledged}"
    );

    // Well inside the 5 s stall valve the reader stays paused.
    sleep(Duration::from_millis(800)).await;
    assert_eq!(process.buffer().2, stalled, "the reader kept reading");

    kill_and_reap(&process).await;
    collector.await.expect("collector task");
}

/// (b) An ack releases the reader and every byte still arrives, in order.
#[tokio::test(flavor = "multi_thread")]
async fn acking_releases_the_reader_with_no_gap() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (snapshot, receiver) = process.subscribe_flow();
    let collector = tokio::spawn(collect_after(snapshot.next_seq, receiver));

    wait_until("the window to fill", Duration::from_secs(10), || {
        process.buffer().2 >= WINDOW
    })
    .await;
    let stalled = process.buffer().2;
    process.ack(stalled);

    wait_until("the reader to resume", Duration::from_secs(10), || {
        process.buffer().2 >= stalled + WINDOW
    })
    .await;

    kill_and_reap(&process).await;
    // The reader is paused at a full window again, with the producer's last
    // output still in the PTY. Keep acknowledging what was published so it
    // drains that and the stream ends with `Closed` — not through the stall
    // valve, whose `Lagged` would stop the collector before the last bytes.
    let mut collector = collector;
    let started = Instant::now();
    let end_seq = loop {
        process.ack(process.buffer().2);
        tokio::select! {
            end = &mut collector => break end.expect("collector task"),
            () = sleep(Duration::from_millis(10)) => {}
        }
    };
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the stream ended through the stall valve, not on its own"
    );
    assert_eq!(
        end_seq,
        process.buffer().2,
        "the delivered stream misses bytes"
    );
}

/// (c) A flow subscriber whose receiver is dropped stops counting, so the
/// reader resumes within one second.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_receiver_releases_the_reader() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (_snapshot, mut receiver) = process.subscribe_flow();

    // Drain while the window fills so the queue never nears the lag limit.
    while process.buffer().2 < WINDOW {
        match tokio::time::timeout(Duration::from_secs(10), receiver.recv()).await {
            Ok(Some(StreamItem::Chunk { .. })) => {}
            other => panic!("the stream ended early: {other:?}"),
        }
    }
    let stalled = process.buffer().2;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(process.buffer().2, stalled, "the reader was not blocked");

    drop(receiver);
    wait_until("the reader to resume", Duration::from_secs(1), || {
        process.buffer().2 > stalled
    })
    .await;

    kill_and_reap(&process).await;
}

/// (c2) Small reads fill a flow subscriber's queue long before the byte
/// window does. The reader pauses there and resumes as the queue drains: a
/// flow subscriber is never dropped for the number of chunks it has queued.
#[tokio::test(flavor = "multi_thread")]
async fn small_chunks_pause_the_reader_instead_of_lagging_a_flow_subscriber() {
    // One byte per write with a pause between, so every read is one chunk.
    let process = PtyProcess::spawn(spec(&[
        "sh",
        "-c",
        "while :; do printf x; sleep 0.002; done",
    ]))
    .expect("spawn the producer");
    let (snapshot, mut receiver) = process.subscribe_flow();

    // Nothing is taken from the queue, so the reader has to stop on its own.
    let mut paused = 0;
    let mut quiet_since = Instant::now();
    wait_until("the reader to pause", Duration::from_secs(4), || {
        let published = process.buffer().2 - snapshot.replay_from;
        if published != paused || published == 0 {
            paused = published;
            quiet_since = Instant::now();
        }
        quiet_since.elapsed() >= Duration::from_millis(400)
    })
    .await;
    assert!(paused < WINDOW, "the window, not the queue, stopped it");

    // Everything queued is output, in order and with no lag notice.
    let mut next_seq = snapshot.replay_from;
    while next_seq < snapshot.replay_from + paused {
        match tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await {
            Ok(Some(StreamItem::Chunk { seq, bytes })) => {
                assert_eq!(seq, next_seq, "stream gap or overlap");
                next_seq += bytes.len() as u64;
            }
            other => panic!("the flow subscriber was dropped: {other:?}"),
        }
    }
    wait_until("the reader to resume", Duration::from_secs(2), || {
        process.buffer().2 - snapshot.replay_from > paused
    })
    .await;

    kill_and_reap(&process).await;
}

/// (d) With no ack for 5 s the flow stream yields `Lagged` and reading resumes.
#[tokio::test(flavor = "multi_thread")]
async fn five_seconds_without_ack_progress_sends_lagged() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (_snapshot, mut receiver) = process.subscribe_flow();

    let start = Instant::now();
    let lagged_at;
    loop {
        match tokio::time::timeout(Duration::from_secs(15), receiver.recv()).await {
            Ok(Some(StreamItem::Chunk { .. })) => continue,
            Ok(Some(StreamItem::Lagged)) => {
                lagged_at = start.elapsed();
                break;
            }
            other => panic!("unexpected stream item: {other:?}"),
        }
    }
    // The stall valve, not the queue-full path, produced this Lagged.
    assert!(
        lagged_at >= Duration::from_secs(4),
        "Lagged arrived after {lagged_at:?}, before the stall valve"
    );
    assert_eq!(receiver.recv().await, None, "items followed Lagged");
    assert_eq!(process.subscriber_count(), 0);

    let mark = process.buffer().2;
    wait_until("output to resume", Duration::from_secs(2), || {
        process.buffer().2 > mark
    })
    .await;

    kill_and_reap(&process).await;
}

/// (e) A lower `ack` after a higher one does not reduce the offset.
#[tokio::test]
async fn a_lower_ack_does_not_move_the_offset_back() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    wait_until("some output", Duration::from_secs(10), || {
        process.buffer().2 > 100
    })
    .await;

    process.ack(100);
    assert_eq!(*process.acknowledged_offset().borrow(), 100);
    process.ack(50);
    assert_eq!(*process.acknowledged_offset().borrow(), 100);

    kill_and_reap(&process).await;
}

/// (f) An ack above the published offset is capped there, so it opens only the
/// usual window of future output.
#[tokio::test(flavor = "multi_thread")]
async fn an_ack_beyond_the_published_offset_is_capped() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (snapshot, receiver) = process.subscribe_flow();
    let collector = tokio::spawn(collect_after(snapshot.next_seq, receiver));

    wait_until("the window to fill", Duration::from_secs(10), || {
        process.buffer().2 >= WINDOW
    })
    .await;
    process.ack(u64::MAX);

    let capped = *process.acknowledged_offset().borrow();
    assert!(
        capped <= process.buffer().2,
        "the acknowledged offset outran the published offset"
    );

    // Reading resumes, then blocks again after one window of future output.
    wait_until("the window to refill", Duration::from_secs(10), || {
        process.buffer().2 >= capped + WINDOW
    })
    .await;
    let restalled = process.buffer().2;
    assert!(restalled <= capped + WINDOW + READ_CHUNK);
    sleep(Duration::from_millis(300)).await;
    assert_eq!(process.buffer().2, restalled, "future output was not gated");

    kill_and_reap(&process).await;
    collector.await.expect("collector task");
}

/// (g) A second `subscribe_flow` keeps the acknowledged offset when the ring
/// has not rolled, and raises it to `replay_from` once it has.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_flow_subscribe_keeps_the_offset_unless_the_ring_rolled() {
    let process = PtyProcess::spawn(spec(&["yes"])).expect("spawn yes");
    let (snapshot, receiver) = process.subscribe_flow();
    let collector = tokio::spawn(collect_after(snapshot.next_seq, receiver));

    wait_until("the window to fill", Duration::from_secs(10), || {
        process.buffer().2 >= WINDOW
    })
    .await;
    let high = process.buffer().2;
    process.ack(high);
    wait_until("output past the ack", Duration::from_secs(10), || {
        process.buffer().2 > high
    })
    .await;

    // The ring has not rolled (total output is far below 1 MiB), so the second
    // subscribe neither lowers the offset nor jumps it to next_seq.
    let (_snapshot2, receiver2) = process.subscribe_flow();
    assert_eq!(*process.acknowledged_offset().borrow(), high);
    assert!(process.buffer().2 > high);
    drop(receiver2);
    collector.abort();

    // Let the ring roll past the acknowledged offset, then subscribe again.
    wait_until(
        "the ring to roll past the offset",
        Duration::from_secs(30),
        || process.buffer().0 > high,
    )
    .await;
    let (snapshot3, _receiver3) = process.subscribe_flow();
    assert!(snapshot3.replay_from > high);
    assert_eq!(
        *process.acknowledged_offset().borrow(),
        snapshot3.replay_from
    );

    kill_and_reap(&process).await;
}

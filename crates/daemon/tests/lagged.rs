use daemon::pty::{PtyProcess, SpawnSpec, StreamItem};
use std::time::Duration;

/// Pace writes so `cat` and the line discipline's echo reach the reader as separate chunks.
async fn feed_lines(process: &PtyProcess, lines: usize) {
    for _ in 0..lines {
        process.write(b"x\n").expect("write to cat");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_subscriber_is_dropped_while_another_gets_every_byte() {
    let process = PtyProcess::spawn(spec(&["cat"])).expect("spawn cat");
    let (_, mut stalled) = process.subscribe();
    let (snapshot, mut active) = process.subscribe();
    assert_eq!(snapshot.next_seq, 0);

    let reader = tokio::spawn(async move {
        let mut stream = Vec::new();
        let mut next_seq = 0;
        loop {
            match active.recv().await.expect("the stream ends with Closed") {
                StreamItem::Chunk { seq, bytes } => {
                    assert_eq!(seq, next_seq);
                    next_seq += bytes.len() as u64;
                    stream.extend_from_slice(&bytes);
                }
                StreamItem::Closed => return stream,
                StreamItem::Lagged => panic!("the active subscriber lagged"),
            }
        }
    });

    feed_lines(&process, 700).await;
    process.write(&[0x04]).expect("send end-of-file to cat");
    let status = process.wait_exit().await.expect("cat exits normally");
    assert_eq!((status.code, status.signal), (0, None));

    let received = reader.await.expect("reader task");
    let (replay_from, retained, _) = process.buffer();
    assert_eq!(replay_from, 0);
    assert_eq!(received, retained, "the active subscriber missed bytes");

    let mut queued = 0;
    loop {
        match stalled.recv().await.expect("the stalled stream ends") {
            StreamItem::Chunk { .. } => queued += 1,
            StreamItem::Lagged => break,
            StreamItem::Closed => panic!("expected Lagged before Closed"),
        }
    }
    assert_eq!(queued, 256, "the queue holds 256 chunks");
    assert_eq!(stalled.recv().await, None, "nothing follows Lagged");
    assert_eq!(process.subscriber_count(), 0);
}

#[tokio::test]
async fn a_dropped_receiver_is_removed_without_error() {
    let process = PtyProcess::spawn(spec(&["cat"])).expect("spawn cat");
    let (_, receiver) = process.subscribe();
    assert_eq!(process.subscriber_count(), 1);
    drop(receiver);

    process.write(b"x\n").expect("write to cat");
    for _ in 0..200 {
        if process.subscriber_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(process.subscriber_count(), 0);

    process.write(&[0x04]).expect("send end-of-file to cat");
    process.wait_exit().await.expect("cat exits normally");
}

fn spec(argv: &[&str]) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir().expect("current directory"),
        cols: 80,
        rows: 24,
    }
}

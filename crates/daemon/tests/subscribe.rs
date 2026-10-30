use daemon::pty::{PtyProcess, SpawnSpec, StreamItem};
use std::time::Duration;

/// A shell loop printing `line0` .. `line{lines-1}`, one `echo` (one PTY write) each.
fn counting_script(lines: usize) -> String {
    format!("i=0; while [ $i -lt {lines} ]; do echo line$i; i=$((i+1)); done")
}

/// What `counting_script` prints through the PTY, which turns `\n` into `\r\n`.
fn counting_output(lines: usize) -> Vec<u8> {
    (0..lines)
        .flat_map(|line| format!("line{line}\r\n").into_bytes())
        .collect()
}

/// macOS can emit `\r\r\n` when the tty output queue backs up, which happens under CPU
/// load: it writes `\r`, finds no room for `\n`, and writes `\r` again on the retry. That
/// is the kernel's output, not the daemon's, so compare modulo that quirk.
fn without_doubled_cr(bytes: &[u8]) -> Vec<u8> {
    let mut cleaned = Vec::with_capacity(bytes.len());
    for (index, byte) in bytes.iter().enumerate() {
        if !(*byte == b'\r' && bytes.get(index + 1) == Some(&b'\r')) {
            cleaned.push(*byte);
        }
    }
    cleaned
}

/// Everything a subscriber saw: snapshot plus chunks, which must continue from the
/// snapshot without a gap or an overlap. `None` if the subscriber lagged, which is the
/// designed outcome when the consumer is starved of CPU.
async fn collect(
    snapshot: daemon::pty::Snapshot,
    mut receiver: tokio::sync::mpsc::Receiver<StreamItem>,
) -> Option<(Vec<u8>, usize)> {
    let mut stream = snapshot.bytes;
    let mut next_seq = snapshot.next_seq;
    let mut chunks = 0;
    loop {
        match receiver.recv().await.expect("the stream ends with Closed") {
            StreamItem::Chunk { seq, bytes } => {
                assert_eq!(seq, next_seq, "chunk does not continue the stream");
                next_seq += bytes.len() as u64;
                stream.extend_from_slice(&bytes);
                chunks += 1;
            }
            StreamItem::Closed => return Some((stream, chunks)),
            StreamItem::Lagged => return None,
        }
    }
}

#[tokio::test]
async fn subscribing_at_random_moments_yields_a_contiguous_stream() {
    const LINES: usize = 5000;
    let script = counting_script(LINES);
    let expected = counting_output(LINES);
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let (mut completed, mut subscribed_mid_stream) = (0, 0);

    // A run whose consumer lagged is skipped: contiguity was still checked up to the lag.
    for _ in 0..300 {
        if completed == 50 {
            break;
        }
        let process = PtyProcess::spawn(spec(&["sh", "-c", &script])).expect("spawn counter");
        // Wait for output to start, then jitter so each run subscribes at a different
        // offset; a fixed delay from spawn would depend on how busy the machine is.
        for _ in 0..1000 {
            if process.buffer().2 > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        tokio::time::sleep(Duration::from_micros((seed >> 33) % 8_000)).await;

        let (snapshot, receiver) = process.subscribe();
        let had_history = !snapshot.bytes.is_empty();
        assert_eq!(snapshot.replay_from, 0, "the output fits in the buffer");
        if let Some((stream, chunks)) = collect(snapshot, receiver).await {
            // `Closed` means the reader is done, so the retained buffer is complete.
            let (_, retained, _) = process.buffer();
            assert_eq!(stream, retained, "the subscriber missed or repeated bytes");
            assert_eq!(
                without_doubled_cr(&stream),
                expected,
                "lines were lost or repeated"
            );
            completed += 1;
            if had_history && chunks > 0 {
                subscribed_mid_stream += 1;
            }
        }
        process.wait_exit().await.expect("wait for the counter");
    }

    assert_eq!(
        completed, 50,
        "too many runs lagged; is the machine overloaded?"
    );
    assert!(
        subscribed_mid_stream > 0,
        "no run subscribed while output was flowing, so the test proved nothing"
    );
}

#[tokio::test]
async fn subscribing_after_exit_yields_the_snapshot_then_closed() {
    let process = PtyProcess::spawn(spec(&["sh", "-c", "echo done"])).expect("spawn echo");
    process.wait_exit().await.expect("wait for echo");

    let (snapshot, receiver) = process.subscribe();
    let (stream, _) = collect(snapshot, receiver).await.expect("one short chunk");
    assert_eq!(stream, b"done\r\n");

    // The reader has finished now, so a late subscriber is told so immediately.
    let (snapshot, mut receiver) = process.subscribe();
    assert_eq!(snapshot.bytes, b"done\r\n");
    assert_eq!(snapshot.replay_from, 0);
    assert_eq!(snapshot.next_seq, 6);
    assert_eq!(receiver.recv().await, Some(StreamItem::Closed));
    assert_eq!(process.subscriber_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn twenty_subscribers_receive_identical_streams() {
    // Each echo is one write, so 150 lines make at most 150 chunks: no subscriber can
    // reach the 256-chunk limit however slowly it runs.
    const LINES: usize = 150;
    let script = format!("sleep 0.3; {}", counting_script(LINES));
    let process = PtyProcess::spawn(spec(&["sh", "-c", &script])).expect("spawn counter");

    let mut readers = Vec::new();
    for _ in 0..20 {
        let (snapshot, receiver) = process.subscribe();
        assert_eq!(
            (
                snapshot.replay_from,
                snapshot.bytes.len(),
                snapshot.next_seq
            ),
            (0, 0, 0),
            "an empty history has replay_from == next_seq"
        );
        readers.push(tokio::spawn(collect(snapshot, receiver)));
    }
    assert_eq!(process.subscriber_count(), 20);

    let expected = counting_output(LINES);
    for reader in readers {
        let (stream, _) = reader
            .await
            .expect("reader task")
            .expect("no subscriber lagged");
        assert_eq!(stream, expected);
    }
    process.wait_exit().await.expect("wait for the counter");
}

fn spec(argv: &[&str]) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir().expect("current directory"),
        cols: 80,
        rows: 24,
    }
}

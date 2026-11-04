use std::collections::VecDeque;

/// Maximum number of output bytes retained for replay.
pub const BUFFER_CAPACITY: usize = 1024 * 1024;

/// A bounded byte buffer whose sequence numbers are cumulative byte offsets.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    bytes: VecDeque<u8>,
    replay_from: u64,
    next_seq: u64,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RingBuffer {
    pub fn new() -> Self {
        Self {
            bytes: VecDeque::with_capacity(BUFFER_CAPACITY),
            replay_from: 0,
            next_seq: 0,
        }
    }

    /// Append bytes and return the sequence number of the first appended byte.
    pub fn append(&mut self, bytes: &[u8]) -> u64 {
        let start_seq = self.next_seq;

        if bytes.len() >= BUFFER_CAPACITY {
            self.bytes.clear();
            self.bytes
                .extend(bytes[bytes.len() - BUFFER_CAPACITY..].iter().copied());
            self.replay_from = start_seq + (bytes.len() - BUFFER_CAPACITY) as u64;
        } else {
            let overflow = self
                .bytes
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(BUFFER_CAPACITY);
            drop(self.bytes.drain(..overflow));
            self.bytes.extend(bytes.iter().copied());
            self.replay_from += overflow as u64;
        }

        self.next_seq += bytes.len() as u64;
        start_seq
    }

    /// The offset the next appended byte will have.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Return the oldest retained offset, retained bytes, and next append offset.
    pub fn snapshot(&self) -> (u64, Vec<u8>, u64) {
        (
            self.replay_from,
            self.bytes.iter().copied().collect(),
            self.next_seq,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RingBuffer, BUFFER_CAPACITY};

    #[test]
    fn empty_buffer_has_matching_offsets() {
        let buffer = RingBuffer::new();

        assert_eq!(buffer.snapshot(), (0, Vec::new(), 0));
    }

    #[test]
    fn append_past_capacity_keeps_newest_bytes_and_advances_offsets() {
        let mut buffer = RingBuffer::new();
        let input = vec![b'a'; BUFFER_CAPACITY - 2];
        assert_eq!(buffer.append(&input), 0);
        assert_eq!(buffer.append(b"bcde"), (BUFFER_CAPACITY - 2) as u64);

        let (replay_from, bytes, next_seq) = buffer.snapshot();
        assert_eq!(replay_from, 2);
        assert_eq!(bytes.len(), BUFFER_CAPACITY);
        assert!(bytes[..BUFFER_CAPACITY - 4]
            .iter()
            .all(|byte| *byte == b'a'));
        assert_eq!(&bytes[bytes.len() - 4..], b"bcde");
        assert_eq!(next_seq, (BUFFER_CAPACITY + 2) as u64);
    }

    #[test]
    fn oversized_append_keeps_its_last_megabyte() {
        let mut buffer = RingBuffer::new();
        let input = vec![b'x'; 3 * BUFFER_CAPACITY];

        assert_eq!(buffer.append(&input), 0);

        let (replay_from, bytes, next_seq) = buffer.snapshot();
        assert_eq!(replay_from, (2 * BUFFER_CAPACITY) as u64);
        assert_eq!(bytes, vec![b'x'; BUFFER_CAPACITY]);
        assert_eq!(next_seq, (3 * BUFFER_CAPACITY) as u64);
    }

    #[test]
    fn offsets_remain_contiguous_across_repeated_evictions() {
        let mut buffer = RingBuffer::new();

        assert_eq!(buffer.append(b"abc"), 0);
        assert_eq!(buffer.append(b"def"), 3);
        assert_eq!(buffer.snapshot(), (0, b"abcdef".to_vec(), 6));

        let bytes = vec![b'z'; BUFFER_CAPACITY];
        assert_eq!(buffer.append(&bytes), 6);
        let (replay_from, retained, next_seq) = buffer.snapshot();
        assert_eq!(replay_from, 6);
        assert_eq!(retained, bytes);
        assert_eq!(next_seq, (BUFFER_CAPACITY + 6) as u64);

        assert_eq!(buffer.append(b"12"), (BUFFER_CAPACITY + 6) as u64);
        let (replay_from, retained, next_seq) = buffer.snapshot();
        assert_eq!(replay_from, 8);
        assert_eq!(retained.len(), BUFFER_CAPACITY);
        assert_eq!(&retained[..2], b"zz");
        assert_eq!(&retained[retained.len() - 2..], b"12");
        assert_eq!(next_seq, (BUFFER_CAPACITY + 8) as u64);
    }
}

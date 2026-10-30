# s1-06 Gap-free output streaming

## Goal

Let any number of consumers attach to a running PTY, receive the retained history plus
live output with no gap or duplicate, and never slow the PTY reader. This is the core of
the snapshot-then-live rule in the [S1 contract](../../../s1-contract.md).

## Scope

In: `PtyProcess::subscribe` and the subscriber registry in `daemon::pty`. Out: socket
frames (s1-08), sessions (s1-07). Depends on: s1-05.

## Behavior

- `subscribe()` returns `(Snapshot{replay_from, bytes, next_seq}, Receiver)`. The
  snapshot is taken and the subscriber registered while holding the same lock the
  reader uses to append, so every byte is either in the snapshot or delivered live.
- Live items are `Chunk{seq, bytes}`, `Lagged`, or `Closed` (process output ended).
- Each subscriber has a bounded queue of 256 chunks. The reader uses a non-blocking
  send under the lock; if the queue is full the subscriber is removed and sent
  `Lagged`. The reader never awaits a subscriber.
- Subscribing after the process exited returns the snapshot and an immediate `Closed`.
- A dropped receiver is removed on the next append without error.

## Edge cases

- Subscribe during a burst of output → still contiguous.
- Stalled subscriber → `Lagged`; other subscribers and the process are unaffected.
- Many subscribers (e.g. 20) on one PTY → all receive identical byte streams.
- Subscriber removed while the reader is appending → no deadlock (lock held only for
  the append plus non-blocking sends).
- Empty history → snapshot with `replay_from == next_seq`.

## Interfaces

`PtyProcess::subscribe() -> (Snapshot, Receiver<StreamItem>)`; `StreamItem::{Chunk,
Lagged, Closed}`. No new dependencies.

## Verification

Integration tests `subscribe` (contiguity, repeated 50 times at random moments) and
`lagged` (stalled subscriber dropped while another gets everything).

## Related context

None.

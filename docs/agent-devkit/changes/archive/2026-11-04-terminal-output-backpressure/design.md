# Terminal output backpressure

## Goal

A program that prints faster than the terminal can draw (`yes`, `cat` of a big file, a
chatty agent) is slowed to the speed of the screen instead of piling output up in the
WebView. Memory stays bounded, the UI stays responsive, Ctrl-C still works, and no output
byte is dropped while the screen keeps up.

## Problem

`ARCHITECTURE.md` (High-output behavior) records the failed `yes` check: the WebView
stopped responding and WebKit held about 4.2 GB. Today every hop is bounded except the
last one. `pty.rs::Shared::publish` never waits for a consumer, and the bridge's
`terminal.rs::deliver` hands each frame to the Tauri `Channel` without learning whether
xterm has parsed it, so the backlog accumulates inside the WebView.

## Scope

- Credit-based flow control for one terminal output stream, end to end: xterm acknowledges
  the bytes it has parsed, and the daemon stops reading the PTY while too many bytes are
  unacknowledged. A blocked PTY blocks the child's `write`, the same as a normal terminal.
- Opt-in per subscription, so every existing subscriber behaves exactly as before.
- Not in scope: terminal-emulator snapshots, repaint-on-overflow, and reattach after the
  app quits (all S4); settings, search, and other S5 items.

## Behavior

- **Window.** A flow-controlled subscription may have at most 256 KiB of output in flight:
  bytes published to the PTY ring buffer minus the highest acknowledged offset. The PTY
  reader checks this before each read and waits while the limit is reached and a live
  flow subscriber exists. Replayed bytes count like live ones: the snapshot is part of
  the same window.
- **Ack.** `terminal.ack { session_id, upto_seq }` carries an absolute byte offset (end of
  the last chunk xterm finished parsing). The daemon keeps the maximum and caps it at the
  current published offset, so repeated, reordered, stale, or future acks cannot grant
  credit for bytes that have not been published.
- **Subscribe.** `terminal.subscribe` gains an optional `flow` flag, default false. The
  acknowledged offset belongs to the session and only moves up. A flow subscribe raises it
  to the snapshot's start (`replay_from`) if it is lower and never lowers it, so it never
  treats unsent replay as acknowledged.
- **Replay pacing.** For a flow subscription `stream_output` sends the snapshot in its
  existing 64 KiB pieces and sends a piece only when its end offset is at most the
  acknowledged offset plus 256 KiB; it waits for acks otherwise. Live chunks that follow
  are already bounded by the reader wait. A 1 MiB replay therefore takes about four window
  round trips, tens of milliseconds at xterm's parsing speed.
- **Frontend.** `TerminalController` passes a callback to `term.write` and, once at least
  64 KiB have been parsed since the last ack, sends one ack for the end offset of the last
  parsed chunk. The 64 KiB threshold is below the window, so unflushed bytes can never
  stall the stream.
- **Stall valve.** If the reader has been waiting 5 s with no ack progress (a hung WebView),
  the daemon removes the flow subscriber with the existing `Lagged` notice and keeps
  reading, so a hung UI never freezes an agent for good. Output that follows is kept only
  in the 1 MiB ring buffer.
- **Resubscribe after a stall.** For a flow subscription the bridge does not resubscribe
  on `Lagged` at once. It waits for the next `terminal.ack` from the webview, which proves
  xterm is parsing again, and then subscribes with `flow` as usual. Without this, every
  resubscribe would restart the window and feed another 256 KiB into a still-hung WebView
  every 5 s. Replay is paced by the same window. If the ring has not rolled past the old
  stream, the existing backlog remains part of that window. If it has rolled, the offset
  advances to `replay_from`; the old WebView backlog can then coexist with one resumed
  window. Because recovery requires at least 64 KiB of parsed output before reattaching,
  the WebView backlog stays below about 0.5 MiB (two windows, including one 16 KiB read
  overrun), however often a stall and recovery repeat. A non-flow `Lagged` resubscribes
  immediately, as today.
- **Dead consumer.** A flow subscriber whose receiver is gone no longer counts, so the
  reader resumes within 250 ms of an unsubscribe or a closed connection.
- **Compatibility.** The bridge sets `flow` and sends acks only to a daemon at version
  0.1.2 or newer, because an older daemon answers an unknown request with `malformed` and
  closes the connection. The daemon crate version becomes 0.1.2. Against an older daemon
  the app behaves as it does today.
- **Not gated.** `terminal.write`, resize, and every control request are dispatched
  without waiting for PTY credit. Their responses still use the existing shared,
  bounded socket-writer queue and may wait behind output already enqueued from other
  sessions.

## Edge cases

- Subscribing again after `Lagged` or a connection change keeps the acknowledged offset.
  The snapshot restarts at `replay_from`, and the bridge's `fresh` drops bytes it already
  delivered. Those dropped bytes produce no new acks, but they are at or below the
  retained offset, so replay pacing never waits for them. Resetting the offset to the start
  of the replay would deadlock here, so it is never reset.
- The bridge captures the ack-watch generation when it receives `terminal.lagged` and waits
  for a later ack event, not for a numerically larger offset. An ack that arrived before the
  lag notice does not release the wait; an ack that arrives after the notice but before the
  wait is registered does. A newly mounted xterm may report offsets below the session's old
  watermark, but its ack event still releases the bridge wait.
- A new xterm (remounted pane) subscribing while the session's acknowledged offset is far
  ahead of `replay_from` may receive the retained snapshot as one burst of at most 1 MiB.
  Until it parses past the old watermark, it may also receive one normal live window
  (256 KiB plus one 16 KiB read). This bounded mount exception happens once per mount; the
  same mounted xterm cannot repeat it under stalls.
- The webview never recovers after a stall: the bridge keeps waiting, the daemon keeps
  reading into the ring buffer with no subscriber, and nothing more reaches the WebView.
  Unsubscribing ends the wait.
- The wait always ends once xterm recovers: the valve fires only with at least 256 KiB
  unacknowledged and the frontend holds back less than 64 KiB, so parsing the backlog
  produces at least one ack.
- Output produced while the bridge waits and rolled out of the 1 MiB ring buffer is lost;
  the stream shows it as a jump in `seq`, as for any lag today.
- An ack above the current published offset is capped there; it cannot disable the window
  for future output.
- A session whose child exits or is stopped while the reader waits: the wait ends when
  acks arrive, the consumer disappears, or the 5 s valve fires; the reader then reads EOF
  and closes the stream as before.
- Hidden panes keep parsing and acking. If WKWebView throttles a minimized window, output
  slows to the acking rate and the valve bounds the worst case; this is checked manually.
- One session is rendered in one pane (ARCHITECTURE.md), so one acknowledged offset per
  session is enough; a second concurrent flow subscriber is not supported.

## Interfaces

- `protocol::message::Request` gains `TerminalAck { id, session_id, upto_seq }` and
  `TerminalSubscribe.flow` (`#[serde(default)]`, omitted when false).
- `daemon::pty::PtyProcess` gains a flow-controlled subscribe, `ack(upto_seq)`, and a way
  to observe the acknowledged offset so `stream_output` can wait for it; `subscribe()` is
  unchanged.
- Bridge: new Tauri command `terminal_ack(id, upto_seq)`; the session's stream task also
  observes it to know when a stalled webview has resumed.
- Frontend: `sessionService.ack(id, uptoSeq)`; `TerminalController` acknowledges after
  `term.write`.

## Verification

- Protocol round trips, including old subscribe JSON without `flow`.
- Daemon tests with an endless producer (`yes`): the reader stops near the window without
  acks, resumes with acks, releases when the receiver drops, and the valve sends `Lagged`
  after 5 s. The existing `lagged.rs` test passes unchanged.
- Daemon IPC test of `terminal.subscribe` with `flow` and `terminal.ack`, including a
  retained buffer larger than the window: the first burst is at most 256 KiB, the rest
  follows acks in order, and a second subscribe after acking does not wait for bytes the
  consumer already has.
- Bridge test with the fake daemon: `flow` and acks are sent only to a supporting version;
  after `Lagged` on a flow stream no new subscribe is sent until a `terminal_ack` arrives;
  a non-flow `Lagged` resubscribes at once.
- Frontend tests: acks coalesce at 64 KiB and carry the correct end offset.
- macOS manual acceptance (no WKWebView driver exists): `yes` then Ctrl-C, `cat` of a large
  file, minimized window during output, and WebKit RSS before and after. Not automatable.

## Reference

A published terminal host's wire protocol uses the same shape: a 256 KiB initial
credit and a `CREDIT` message for consumed bytes, with stalls never blocking the control
channel. Only that published design was read; no code is copied.

## Related context

- [Architecture overview](../../../../docs/llm/architecture/overview.md)

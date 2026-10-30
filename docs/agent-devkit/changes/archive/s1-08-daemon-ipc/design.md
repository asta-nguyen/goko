# s1-08 Daemon IPC wiring

## Goal

Expose sessions and terminals over the authenticated socket so any client can create a
session, stream its output, send input, and stop it, then prove the whole daemon with an
end-to-end test. Implements the command and event parts of the
[S1 contract](../../../s1-contract.md).

## Scope

In: extend `server::dispatch`, pass `SessionManager` into `serve`, shutdown hook in
`gokod` main, end-to-end test. Out: the Tauri client (s1-09). Depends on: s1-04, s1-06,
s1-07.

## Behavior

- `session.create/list/get/stop` map to `SessionManager`. Invalid input →
  `invalid_argument`; unknown id → `not_found`; spawn failure returns the session in
  state `failed`.
- `events.subscribe` registers the connection; it then receives `session.updated` and
  `session.exited` pushes (full `SessionInfo`). Unsubscribed connections receive none.
- `terminal.subscribe{id}` replies `{replay_from, next_seq}`, sends the snapshot and then
  live chunks as kind-1 frames with correct `seq`. `Lagged` becomes a
  `terminal.lagged` event and removes the subscription; the client re-subscribes.
  `terminal.unsubscribe` and disconnect remove subscriptions and never touch the session.
- `terminal.write{id, data_b64}` decodes and writes; invalid base64 → `malformed`; write
  to an exited session → `invalid_argument` with a message. `terminal.resize` uses the
  s1-05 bounds.
- `gokod` main calls `SessionManager::shutdown()` on SIGTERM/SIGINT before removing the
  socket.

## Edge cases

- Second subscriber mid-stream gets snapshot + live without gap or duplicate.
- Stalled client gets `terminal.lagged`; other clients and the session are unaffected.
- Client killed mid-stream → session keeps running, subscription cleaned up.
- Subscribing to an exited session → snapshot then end of stream.
- Daemon SIGTERM with live sessions → all session processes are gone afterward.

## Interfaces

`server::serve(listener, token, Arc<SessionManager>)`; per-connection writer task that
multiplexes responses, events, and output frames onto one socket write half.

## Verification

Integration tests `session_ipc`, `subscribe_ipc`, `terminal_io_ipc`, and `e2e` (real
`gokod`, temp `GOKO_HOME`). `cargo test --workspace`, clippy, and fmt all pass.

## Related context

None.

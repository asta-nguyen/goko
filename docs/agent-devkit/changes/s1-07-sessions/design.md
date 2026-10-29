# s1-07 Session manager

## Goal

In-memory sessions with an enforced state machine, idempotent stop, and clean shutdown
of every process. Used by the IPC layer in s1-08. Implements the session rules of the
[S1 contract](../../s1-contract.md).

## Scope

In: `daemon::session::{transition, SessionManager}`. Out: sockets and request routing
(s1-08), persistence (S2). Depends on: s1-03, s1-05, s1-06.

## Behavior

- **States used:** created, starting, running, stopping, exited, failed. Allowed edges:
  created→starting→running→stopping→exited; running→exited; created|starting→failed;
  stopping→exited. Anything else, including leaving exited/failed, is rejected.
- **create(spec):** validates (non-empty argv, existing directory cwd, cols/rows in
  1..=1000, title defaults to `argv[0]` basename), assigns a UUID, and drives
  created→starting→running before returning. Spawn failure ends in `failed{error}` and
  is returned as a session, not an RPC error; invalid input is `invalid_argument`.
- **Exit watcher:** when the process ends, state becomes `exited{exit_code}` (also from
  `stopping`) and `ended_at` is set.
- **stop(id, force):** running → `stopping` and `terminate`; with force → `kill`.
  Exited/failed sessions return ok without signalling. Concurrent stops send one signal.
- **list / get:** ordered by creation time; exited and failed sessions remain.
- **Events:** every state change broadcasts the full `SessionInfo` (tokio broadcast,
  capacity 64). A lagging receiver re-lists instead of replaying.
- **shutdown():** `terminate` every live process, wait up to 2 s, then `kill` the rest.

## Edge cases

- Process exits while `stopping` → `exited`, not an error.
- `stop` with an unknown id → `NotFound`.
- Two simultaneous `stop` calls → exactly one SIGTERM.
- Spawn failure leaves a listed `failed` session with a message.
- `shutdown` with no sessions returns immediately.

## Interfaces

`transition(SessionState, Transition) -> Result<SessionState, InvalidTransition>`;
`SessionManager::{new, create, list, get, stop, shutdown, subscribe_events, pty}`
(`pty(id)` hands s1-08 the `PtyProcess` for terminal I/O); `SessionError::{InvalidArgument,
NotFound}`.

## Verification

Unit test for the transition graph; integration test `session_manager` (no socket) using
`cat` and `sh`; clippy and fmt clean.

## Related context

None.

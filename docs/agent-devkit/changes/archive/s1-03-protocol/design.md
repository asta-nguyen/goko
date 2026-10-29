# s1-03 Protocol crate

## Goal

The complete, tested wire contract between daemon and clients: framing, session model,
and control messages. Everything above this plan only imports these types. Implements
the framing and message sections of the [S1 contract](../../../s1-contract.md).

## Scope

In: `crates/protocol` modules `frame`, `session`, `message`. Out: sockets, auth
checks, any I/O (those are s1-04). Depends on: s1-01.

## Behavior

- **Frame:** `u32` big-endian payload length, `u8` kind, payload. Kind `0` JSON
  control message, kind `1` terminal output `[16B session id][u64 BE seq][bytes]`.
  Max payload 1 MiB, checked before allocation.
- **Session model:** `SessionId` is a UUID serialized as a string. Session timestamps
  are Unix epoch milliseconds (`i64`). `SessionState` is the full target enum
  (created, starting, running, waiting_for_input, idle, stopping, exited, failed,
  disconnected, interrupted, unknown) as snake_case. `SessionInfo` = id, title, argv,
  cwd, state, exit_code, error, started_at, ended_at.
- **Messages:** `Hello{protocol_version, token}` / `HelloOk{daemon_version}`. Requests
  carry a numeric `u64` correlation `id`: `system.health`, `system.version`, `events.subscribe`,
  `session.create{argv, cwd, cols, rows, title}`, `session.list`, `session.get`,
  `session.stop{id, force}`, `terminal.write{id, data_b64}`,
  `terminal.resize{id, cols, rows}`, `terminal.subscribe{id}`,
  `terminal.unsubscribe{id}`. Responses echo `id` with a result or `ProtoError`.
  Events: `session.updated`, `session.exited`, `terminal.lagged`.
- **Errors:** `ErrorCode` = auth_failed, version_unsupported, frame_too_large,
  malformed, invalid_argument, not_found, spawn_failed, internal. `ProtoError` =
  code, message, optional recovery hint.

## Edge cases

- Length prefix above 1 MiB → `FrameTooLarge` without allocating.
- Truncated input → "incomplete" result, never a panic; unknown kind → `Malformed`.
- Request without `id`, unknown request type, or unknown state string → parse error.
- Output frame shorter than its 24-byte header → `Malformed`.

## Interfaces

Public: `frame::{FrameKind, encode, decode, encode_output, decode_output}`,
`session::{SessionId, SessionState, SessionInfo}`,
`message::{Hello, HelloOk, Request, Response, Event, ErrorCode, ProtoError}`.
Dependencies: `serde`, `serde_json`, `uuid` only.

## Verification

`cargo test -p protocol` covers every case above; `cargo clippy -p protocol -- -D warnings`.

## Related context

None.

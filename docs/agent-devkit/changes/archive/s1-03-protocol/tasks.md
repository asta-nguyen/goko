# s1-03 Protocol crate tasks

## Design

[s1-03 Protocol crate](design.md)

## Global Constraints

- Max frame payload 1 MiB, enforced in the decoder before allocation.
- `seq` in output frames is the cumulative byte offset of the chunk's first byte.
- The crate does no I/O and depends only on `serde`, `serde_json`, `uuid`.

## Impact map

```text
Entry: `crates/protocol/src/lib.rs::PROTOCOL_VERSION` exists; frame, session, and message modules are new
Flow: no existing encode/decode flow; planned flow: daemon and bridge call encode/decode on bytes read from the socket
State changes: none found
External effects: none found
Change candidates: crates/protocol/src/{lib,frame,session,message}.rs, crates/protocol/Cargo.toml, Cargo.lock
Verification: cargo test -p protocol; cargo clippy -p protocol -- -D warnings
Verified at: no commit exists
```

## Tasks

### Task 1 — Frame codec

Files: crates/protocol/src/frame.rs::{FrameKind, encode, decode, encode_output, decode_output}
Interfaces: produces the framing used by the daemon server (s1-04) and the Tauri client (s1-09).
Change: layout as in the design. `decode` returns the frame and bytes consumed, or `Incomplete`; it rejects length > 1 MiB with `FrameTooLarge` before allocating and an unknown kind with `Malformed`. `encode_output`/`decode_output` build and parse the kind-1 payload; a payload under 24 bytes is `Malformed`.
Verify: `cargo test -p protocol frame` — JSON and output frames round-trip; length 1 MiB + 1 → `FrameTooLarge`; truncated input → `Incomplete`; kind 9 → `Malformed`; 23-byte output payload → `Malformed`.
Progress: done
Result: `cargo test -p protocol frame` — 6 frame tests pass: JSON and output frames round-trip; length 1 MiB + 1 → `FrameTooLarge` with no payload present; truncated input → `Incomplete`; kind 9 → `Malformed`; 23-byte output payload → `Malformed`.

### Task 2 — Session types

Files: crates/protocol/src/session.rs::{SessionId, SessionState, SessionInfo}
Interfaces: produces the session model used by the daemon (s1-07) and mirrored in TypeScript (s1-10).
Change: `SessionId` wraps a UUID and serializes as a string. `SessionState` has the 11 states from the design with `#[serde(rename_all = "snake_case")]`. `SessionInfo` has the fields in the design; `exit_code`, `error`, `ended_at` are optional.
Verify: `cargo test -p protocol session` — every state serializes to its snake_case name and round-trips; `"bogus"` fails to deserialize; a `SessionInfo` with no exit code round-trips.
Progress: done
Result: `cargo test -p protocol session` — 4 tests pass: all 11 states serialize to snake_case and round-trip; `"bogus"` fails to deserialize; a `SessionInfo` with no exit code round-trips; `SessionId` is a UUID string and keeps its 16 frame bytes.

### Task 3 — Control messages and errors

Files: crates/protocol/src/message.rs::{Hello, HelloOk, Request, Response, Event, ErrorCode, ProtoError}
Interfaces: consumes `SessionInfo` and `SessionId` from Task 2; produces the JSON messages carried in kind-0 frames.
Change: define the requests, responses, events, and error codes listed in the design. `Request` is a tagged enum on `type` with a required `id`. `ProtoError` carries `code`, `message`, and optional `hint`.
Verify: `cargo test -p protocol message` — each request, response, and event round-trips through JSON; a request missing `id` and an unknown `type` both fail to parse; `ErrorCode` serializes as snake_case.
Progress: done
Result: `cargo test -p protocol message` — 7 tests pass: every request, response, and event round-trips; a request without `id` and an unknown `type` fail to parse; a response with both or neither of `result`/`error` fails; `ErrorCode` is snake_case. Requests name the session field `session_id` because `id` is the correlation id. `cargo clippy -p protocol -- -D warnings` and `cargo fmt --check` are clean.

## Files inspected, no change

- `Cargo.toml` — the workspace already includes `protocol`; no root workspace edit is planned.
- `crates/daemon/src/main.rs` — the version-only CLI has no current protocol callers; daemon use begins in s1-04.

## Approval Gate

Status: approved

## Decision Log

- JSON request/response correlation IDs are numeric (`u64`), and session timestamps are Unix epoch milliseconds (`i64`). `SessionId` remains a UUID serialized as a string, matching the S1 contract and its 16-byte terminal frame identity.

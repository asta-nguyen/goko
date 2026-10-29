# s1-04 Daemon server and lifecycle

## Goal

`gokod` runs as a single instance, listens on an authenticated local socket, answers
health/version requests, and shuts down cleanly. No sessions or PTYs yet. Implements
the daemon-start, handshake, and framing parts of the [S1 contract](../../s1-contract.md).

## Scope

In: `daemon::{paths, lock, auth, server}` and the real `gokod` main. Out: sessions,
PTY, terminal commands (s1-05 to s1-08). Depends on: s1-01, s1-03.

## Behavior

- **Paths:** `Paths::resolve()` uses `GOKO_HOME`, else `~/Library/Application Support/Goko`.
  It creates the directory with mode 0700 and exposes `daemon.lock`, `daemon.sock`,
  `daemon.token`, `daemon.log`.
- **Lock:** non-blocking exclusive `flock` on `daemon.lock`, held until dropped.
- **Token:** 32 bytes from `/dev/urandom` written as 64 hex chars to `daemon.token`
  (0600) after the lock is held; verification is constant time.
- **Server:** removes a stale `daemon.sock`, binds with mode 0600, accepts any number of
  clients. First frame must be `Hello`; wrong token → `auth_failed`, unsupported
  version → `version_unsupported`, error frame then close. Nothing is served before auth.
- **Dispatch:** `system.health` → ok with uptime; `system.version` → daemon and protocol
  versions; anything else → `not_found` for now.
- **main:** lock held → print `gokod: already running` and exit 0. Otherwise log with
  `tracing` to `daemon.log`, serve; on SIGTERM/SIGINT stop accepting, remove the socket
  file, exit 0.

## Edge cases

- Second `gokod` while one runs → exits 0, the first is unaffected.
- Stale socket from a crashed daemon → removed and rebound.
- Wrong token, wrong version, malformed JSON, oversized or truncated frame → typed error
  or close for that connection only; others keep working.
- Token file unreadable by a client → the client reports it (s1-09), not the daemon.
- Client disconnects mid-handshake → connection task ends without logging an error.

## Interfaces

`Paths`, `InstanceLock::acquire`, `auth::{create_token, Token::verify}`,
`server::{bind, serve, dispatch}`. `serve` later takes a `SessionManager` (s1-08).
Dependencies: `tokio`, `tracing`, `tracing-subscriber`, `libc` (for `flock`/signals).

## Verification

`cargo test -p daemon` for paths, lock, auth; integration tests `handshake`, `dispatch`,
`lifecycle` (real `gokod` binary in a temp `GOKO_HOME`); clippy and fmt clean.

## Related context

None.

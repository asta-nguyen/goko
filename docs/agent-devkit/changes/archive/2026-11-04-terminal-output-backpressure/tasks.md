# Terminal output backpressure tasks

## Design

[Terminal output backpressure](design.md)

## Global Constraints

- Flow window is 256 KiB of unacknowledged output, replayed snapshot bytes included; frontend acks at 64 KiB of parsed bytes.
- The acknowledged offset belongs to the session and never decreases; a flow subscribe raises it to `replay_from` and never resets it. `stream_output` sends a flow snapshot piece only when its end offset is at most acknowledged + 256 KiB.
- `terminal.ack.upto_seq` is an absolute end offset; the daemon keeps the maximum, ignores lower values, and caps future offsets at the current published offset.
- Flow control is opt-in through `terminal.subscribe.flow` (default false); a subscriber without it behaves exactly as today (256-chunk queue, then `Lagged`).
- While the consumer keeps acknowledging, no output byte is dropped. After 5 s without ack progress the daemon falls back to the existing `Lagged` path.
- After a flow stall resumes, pending WebView bytes plus newly delivered replay/live output stay below about 0.5 MiB (two windows plus the single 16 KiB read overrun); a newly mounted xterm with a far-ahead session watermark may receive one bounded 1 MiB replay burst plus one live window.
- `terminal.write`, resize, and control requests are never gated by credit.
- After a stall `Lagged` on a flow stream, the bridge resubscribes only after an ack event received after the lag notice; the offset need not be numerically greater than the previous value. A non-flow `Lagged` resubscribes at once.
- The bridge sets `flow` and sends acks only to a daemon at version 0.1.2 or newer; ack errors are ignored by the UI.
- The daemon crate version becomes 0.1.2.

## Impact map

Entry: `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.mount` (subscribe and `term.write`) and `crates/daemon/src/pty.rs::spawn_reader` (the PTY read loop)

Flow: PTY child output → `spawn_reader` → `Shared::publish` → subscriber queue → `server.rs::stream_output` → socket → `daemon_client.rs::route_output` → `terminal.rs::forward`/`deliver` → Tauri `Channel` → `sessionService.subscribe` → `TerminalController` → `term.write`. Planned return path (pending approval): `term.write` callback → `sessionService.ack` → `terminal_ack` command → `terminal.ack` request → `server.rs::dispatch` → `PtyProcess::ack` → wakes the reader gate

State changes: planned (pending approval) per-session acknowledged offset and flow-subscriber marker in `pty.rs::Shared`; frontend counter of parsed-but-unacked bytes; no SQLite or `GOKO_HOME` change

External effects: planned (pending approval) one more request type on `daemon.sock`; the PTY reader thread pauses, which blocks the child's `write`; daemon crate version 0.1.2

Change candidates (pending approval): `crates/protocol/src/message.rs`; `crates/daemon/src/pty.rs`; `crates/daemon/src/server.rs`; `crates/daemon/Cargo.toml` and `Cargo.lock`; `apps/desktop/src-tauri/src/daemon_client.rs`; `apps/desktop/src-tauri/src/terminal.rs`; `apps/desktop/src-tauri/src/lib.rs`; `apps/desktop/src/services/session.ts`; `apps/desktop/src/features/terminal/TerminalController.ts`; their tests; `ARCHITECTURE.md`; `CHANGELOG.md`

Verification (pending approval): `cargo test --workspace`; `cargo fmt --check`; `cargo clippy --workspace -- -D warnings`; `pnpm typecheck`; `pnpm test`; macOS manual acceptance for `yes`, Ctrl-C, large `cat`, minimized window, RSS

Verified at: `89b2f215dc4bd4e6038fcebce01bbdd48a72ea93`

## Tasks

### Task 1 — Protocol: `terminal.ack` and the `flow` flag

Files: `crates/protocol/src/message.rs::Request`

Interfaces: produces `Request::TerminalAck { id, session_id, upto_seq: u64 }` (wire name `terminal.ack`) and `Request::TerminalSubscribe { id, session_id, flow: bool }`; later tasks consume both.

Change: add the two shapes. `flow` uses `#[serde(default)]` and is omitted from JSON when false, so existing clients and fixtures are unchanged.

Verify: round-trip tests in `message.rs`: `terminal.ack` encodes and decodes; `{"type":"terminal.subscribe","id":1,"session_id":…}` without `flow` decodes with `flow == false`; serializing `flow == false` omits the field; a flow subscribe round-trips with `flow == true`; the existing request round-trip test still passes. `cargo test -p protocol`.

Progress: done
Result: `cargo test -p protocol` — 20 passed, 0 failed (baseline 17; 3 new tests cover `terminal.ack` encode/decode, missing `flow` decoding to false, `flow` omitted when false and round-tripping when true; `every_request_round_trips_with_numeric_id` still passes and now covers both new shapes). `cargo test --workspace`, `cargo fmt --check`, and `cargo clippy --workspace -- -D warnings` also pass. Compile fallout from the enum change was kept green with the smallest fixes the workspace needed: `flow: false` at the `attach` literal in `apps/desktop/src-tauri/src/terminal.rs` (wire-identical; Task 4 negotiates it) and a `Request::TerminalAck` arm in `crates/daemon/src/server.rs::dispatch` that returns `not_found` for an unknown session and otherwise accepts as a no-op (Task 3 wires `PtyProcess::ack`).

### Task 2 — Daemon: flow gate in the PTY reader

Files: `crates/daemon/src/pty.rs::Shared`; `crates/daemon/src/pty.rs::spawn_reader`; `crates/daemon/src/pty.rs::PtyProcess::subscribe`; new `crates/daemon/tests/flow.rs`

Interfaces: consumes nothing from earlier tasks. Produces `PtyProcess::subscribe_flow() -> (Snapshot, Receiver<StreamItem>)`, `PtyProcess::ack(upto_seq: u64)`, and a way to observe the acknowledged offset (for example a `tokio::sync::watch::Receiver<u64>`) for Task 3. `subscribe()` is unchanged.

Change: track the highest acknowledged offset and which subscribers are flow-controlled. `subscribe_flow` registers a flow subscriber and raises the acknowledged offset to the snapshot's `replay_from` if it is lower; it never lowers it and never sets it to `next_seq`. Before each `read`, `spawn_reader` waits (without holding the `Shared` lock across the wait; it re-checks at least every 250 ms) while some live flow subscriber exists and published-minus-acknowledged is at least 256 KiB. `ack` raises the offset (never lowers it), caps it at the current published offset, and wakes the reader. A subscriber whose receiver is closed does not count. If the wait lasts 5 s without the acknowledged offset moving, send `Lagged` to the flow subscribers, remove them, and continue reading (the existing `Shared::publish` lag path).

Verify: tests in `flow.rs` using an endless producer (`yes`): (a) a flow subscriber that never acks stops the reader with published bytes at most 256 KiB plus one 16 KiB read past the acknowledged offset; (b) after `ack`, publishing resumes and every byte arrives in order with no gap; (c) dropping the receiver releases the reader within 1 s; (d) with no ack for 5 s the stream yields `Lagged`; (e) a lower `ack` after a higher one does not reduce the offset; (f) an ack above the published offset cannot open more than the 256 KiB window plus one 16 KiB read of future output; (g) after acks to offset X, a second `subscribe_flow` leaves the offset at X (not lower, not `next_seq`), and on a session whose ring has rolled the offset becomes `replay_from`. The existing `crates/daemon/tests/lagged.rs` passes unchanged. `cargo test -p daemon`.

Progress: done
Result: `cargo test -p daemon` — 87 passed, 0 failed (baseline 80; 7 new tests in `crates/daemon/tests/flow.rs` cover: reader stops with published ≤ acknowledged + 256 KiB + one 16 KiB read; ack releases with a contiguous gapless stream; receiver drop releases the reader within 1 s; `Lagged` after 5 s of no ack progress and the stream ends; lower ack cannot reduce the offset; `u64::MAX` ack is capped and future output stays gated; second `subscribe_flow` keeps the offset and a rolled ring raises it to `replay_from`). `lagged.rs` passes unchanged. `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` pass. Implementation: `Shared` gained `acknowledged` + `acknowledged_watch` and `subscribers` became `Vec<Subscriber>` with a `flow` flag; `PtyProcess` gained `subscribe_flow`, `ack`, and `acknowledged_offset` (`watch::Receiver<u64>` for Task 3) plus a `Condvar` the reader waits on; `wait_for_flow_credit` gates each `read` with a 250 ms re-check and the 5 s stall valve. `RingBuffer::next_seq()` accessor added for the window/cap checks. `subscribe()` unchanged for non-flow callers.

### Task 3 — Daemon server: handle the ack and the flow flag; bump version

Files: `crates/daemon/src/server.rs::Client::dispatch`; `crates/daemon/src/server.rs::Client::terminal_subscribe`; `crates/daemon/Cargo.toml`; `Cargo.lock`; `crates/daemon/tests/subscribe_ipc.rs`

Interfaces: consumes Task 1 requests and Task 2 `subscribe_flow`/`ack`.

Change: `terminal.subscribe` with `flow: true` calls `subscribe_flow`; otherwise `subscribe`. For a flow subscription `stream_output` sends each 64 KiB snapshot piece only when its end offset is at most the acknowledged offset plus 256 KiB, waiting on the observable offset from Task 2 otherwise (and ending if the connection closes); live chunks are sent as today. A non-flow subscription is unchanged. `terminal.ack` calls `PtyProcess::ack` for the session and replies with an empty result; an unknown session returns `not_found`; an ack without a flow subscription is accepted and has no effect. Set the daemon crate version to 0.1.2.

Verify: IPC test over the real socket: a flow subscribe with a `yes` session stops delivering output near 256 KiB without acks, resumes after a `terminal.ack` for the received end offset, an ack for an existing session without a flow subscription succeeds without changing flow state, and an ack for an unknown session id returns `not_found`. Replay test: with more than 1 MiB retained (producer finished or no flow subscriber yet), a flow subscribe delivers at most 256 KiB before any ack, then the rest in order as acks arrive, with total bytes equal to the retained buffer. Re-attach test: after acking up to `next_seq` and subscribing flow again, the whole snapshot arrives with no further ack, and after acking only to a lower offset X the second snapshot delivers up to X plus 256 KiB and then waits. With a producer still running on a remounted xterm, the exception is limited to the 1 MiB snapshot plus one 256 KiB live window and one 16 KiB read. `cargo test -p daemon`; the version assertions use `CARGO_PKG_VERSION` and still pass.

Progress: done
Result: `cargo test -p daemon` — 95 passed, 0 failed (subscribe_ipc grew 8→15; new tests cover: flow subscribe stalls at exactly one 256 KiB window until a `terminal.ack` releases exactly one more; an ack on a non-flow subscription returns `{}` and gates nothing; an ack for an unknown session returns `not_found`; a finished producer's 1 MiB replay delivers exactly 256 KiB then the rest in order as acks arrive, total equal to the retained buffer; a second flow subscribe after acking `next_seq` gets the whole snapshot with no ack; after a partial ack to `replay_from`+64 KiB the second snapshot delivers that offset plus one window then waits; a remount with the acknowledged offset at the published end gets the ~1 MiB snapshot burst plus at most one 256 KiB+16 KiB live window before the gate). `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` pass. Implementation: `FLOW_WINDOW` is now `pub(crate)`; `Output` carries `credit: Option<watch::Receiver<u64>>`; `terminal_subscribe` branches to `subscribe_flow` and attaches `acknowledged_offset`; `stream_output` paces snapshot pieces through `has_credit` (waits on the watch until piece end ≤ acknowledged + `FLOW_WINDOW`, ends when the producer drops the sender or the task is aborted on disconnect); the `TerminalAck` arm calls `sessions.pty` + `PtyProcess::ack` (`not_found` for unknown ids, `invalid_argument` when no process exists); daemon version bumped to 0.1.2 and `Cargo.lock` updated. Note: `sessions.pty` returns `invalid_argument` rather than `{}` for a failed session with no process.

### Task 4 — Bridge: negotiate by version and forward acks

Files: `apps/desktop/src-tauri/src/daemon_client.rs::supports_flow_control`; `apps/desktop/src-tauri/src/terminal.rs::attach`; `apps/desktop/src-tauri/src/terminal.rs::terminal_ack`; `apps/desktop/src-tauri/src/lib.rs::run`; `apps/desktop/src-tauri/src/terminal/tests.rs`

Interfaces: consumes Task 1 requests. Produces the Tauri command `terminal_ack(id, upto_seq)` for Task 5.

Change: `supports_flow_control(version)` is true for 0.1.2 and newer (same parsing as `supports_live_cwd`). `attach` sends `flow: true` only when the connected daemon supports it. `terminal_ack` forwards `terminal.ack` to the daemon when supported and returns without error otherwise; a daemon error or disconnect is swallowed (the UI ignores ack failures). It also publishes every ack event and its `upto_seq` to a per-session `tokio::sync::watch`; the watch is an event signal and does not discard lower offsets, while the daemon still keeps its maximum. When `forward` receives `terminal.lagged` on a flow attach, it captures the watch generation at that moment and `stream` waits for a later ack event before calling `attach` again. An ack received before the lag notice does not release the wait; one received after the notice but before the wait is registered does. A newly mounted xterm can publish an offset below the old session watermark and still release the wait. The wait ends if the stream task is aborted by unsubscribe or subscribe. A non-flow attach, a closed connection, and `fresh` behave as today. Register the command in `lib.rs`.

Verify: tests with the fake daemon: a 0.1.2 daemon receives `terminal.subscribe` with `flow: true` and then a `terminal.ack` with the offset; a 0.1.1 daemon receives a subscribe without `flow` and no ack request when `terminal_ack` is called; `terminal_ack` for a missing session does not return an error; after the fake daemon sends `terminal.lagged` on a flow stream no second `terminal.subscribe` arrives within 1 s, then one arrives after a `terminal_ack`; an ack received before `Lagged` does not release the wait; an ack received after `Lagged` but before the wait is registered releases it immediately; an ack from a remounted xterm with a lower offset still releases the wait; unsubscribing during the wait ends the task; `terminal.lagged` on a non-flow (0.1.1) stream resubscribes without waiting; a closed 0.1.2 connection resubscribes without waiting for an ack. `cargo test -p desktop`.

Progress: done
Result: `cargo test -p desktop` — 109 passed, 0 failed (terminal tests grew 8→15; new tests cover: 0.1.2 gets `flow: true` plus a forwarded `terminal.ack` with the offset; 0.1.1 gets a plain subscribe and no ack request; `terminal_ack` swallows a `not_found` reply and needs no live daemon; a lagged flow stream holds at one subscribe for 1 s then a `terminal_ack` — with a lower, remounted-view offset — releases a second; an ack wired before the lag notice releases nothing while the post-lag ack does; unsubscribing mid-wait ends the task; a closed 0.1.2 connection re-subscribes without any ack). `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` pass. Implementation: `version_at_least` is shared by `supports_live_cwd` (≥0.1.1) and `supports_flow_control` (≥0.1.2); `Bridge` keeps a per-session `watch::Sender<u64>` in `acks`, exposed via `ack_events` (a new receiver treats the current offset as seen, so only a post-subscribe ack fires `changed`) and `publish_ack` (`send_replace`, so even a lower offset is a new event); `terminal_ack` always publishes, forwards through `Request::TerminalAck` only when the live client supports flow, and drops the reply. `Attached.flow` marks the attach; `forward` returns `Now`/`AfterAck`/`Closed`, and `AfterAck` captures `ack_events` at the lag moment before `stream` awaits `changed` — a pre-lag ack is already seen and cannot release, while an aborted task never resubscribes. Non-flow lag stays an immediate `Now`. `terminal_ack` is registered in `lib.rs` and returns `Ok(())` because Tauri requires async commands with `State` to return a `Result`.

### Task 5 — Frontend: acknowledge parsed output

Files: `apps/desktop/src/services/session.ts::sessionService`; `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.mount`; `apps/desktop/src/services/session.test.ts`; `apps/desktop/src/features/terminal/TerminalController.test.ts`

Interfaces: consumes the `terminal_ack` command from Task 4.

Change: add `sessionService.ack(id, uptoSeq)` calling `terminal_ack`, and include `ack` in the `Service` type the controller uses. In the subscribe callback, call `term.write(data, callback)`; in the callback add `data.length` to a parsed counter and remember `seq + data.length`; once the counter reaches 64 KiB, send one ack for that end offset, ignore its failure, and reset the counter. Nothing is sent after `dispose()`. The fake terminal in the test calls the write callback.

Verify: `TerminalController` tests: 63 KiB parsed sends no ack; crossing 64 KiB sends exactly one ack whose offset is the end of the last parsed chunk; the counter resets; a rejected ack does not throw; no ack after dispose. `session.test.ts`: `ack` invokes `terminal_ack` with `{ id, uptoSeq }`. `pnpm typecheck` and `pnpm test`.

Progress: done
Result: direct `pnpm typecheck` passes (`tsc --noEmit`, exit 0; the rtk-rewritten variant is not this result). `pnpm vitest run` — 23 files, 234 tests, all pass (TerminalController tests grew 29→34 covering: 63 KiB parsed acks nothing, crossing the threshold acks once at the last chunk's absolute end offset, the counter restarts and a second 64 KiB acks again, one >128 KiB chunk still yields a single ack, a rejected ack is ignored and parsing continues, a hidden pane keeps acking, nothing is sent after dispose; `session.test.ts` asserts `ack` invokes `terminal_ack` with `{ id, uptoSeq }`). One earlier run saw 7 test files fail to spawn vitest workers (machine-load timeouts, environment 99% of 930 s); the re-run in 4.42 s is the authoritative result. Implementation: `sessionService.ack(id, uptoSeq)` wraps `terminal_ack`; `Service` picks up `ack`; the subscribe callback passes a callback to `term.write` so `parsedChunk` counts bytes only after xterm parses them, and at ≥64 KiB sends one coalesced ack for `seq + length` with `.catch(noop)`. The fake terminal now invokes the optional write callback.

### Task 6 — Documentation

Files: `ARCHITECTURE.md` (Replay buffer and seq, High-output behavior, and daemon protocol sections); `CHANGELOG.md`

Interfaces: consumes the final behavior of Tasks 1-5.

Change: update the replay section's unconditional reader behavior to distinguish flow and non-flow subscriptions, and document that flow replay is paced by the same window; replace the "end-to-end output backpressure is not implemented in S1" text with the credit-window behavior, the 256 KiB window, the 5 s valve, the version requirement, and the measured result from Task 7. Document `terminal.ack` and `terminal.subscribe.flow` next to the existing terminal requests. Add an entry under `[Unreleased]` in `CHANGELOG.md`. The `docs/llm/architecture/overview.md` page is high-level and makes no claim about flow control, so no wiki change applies.

Verify: grep `ARCHITECTURE.md` to confirm the removed "not implemented" text and unconditional "PTY reader never waits for a client" claim are gone, `terminal.ack` and `terminal.subscribe.flow` are documented, and links resolve.

Progress: done
Result: `ARCHITECTURE.md` updated in three places: the request list now names `terminal.ack` (`upto_seq`) and `terminal.subscribe`'s `flow` option; the Replay buffer section distinguishes non-flow subscribers (only kind a pre-0.1.2 daemon understands; reader never waits) from flow subscribers (256 KiB window over published-minus-acknowledged, replay paced by the same window, monotonic `upto_seq` capped at the published end, 5 s `Lagged` valve, bridge waits for a post-lag ack before resubscribing); the High-output section replaces the "not implemented in S1" verdict with the negotiated gate and keeps the pre-gate 4.2 GB `yes` observation as history. grep confirms no "not implemented" text remains and the surviving "never waits for a client" sentence is scoped to non-flow subscriptions. `CHANGELOG.md` gained an `[Unreleased]`/`Added` entry. Links verified — one pre-existing stale link (`changes/2026-11-02-worktree-ux/` was archived) now points at `changes/archive/`; all targets resolve. `docs/llm/architecture/overview.md` needed no change. The Task 7 measurement, if any, is appended to the High-output section.

### Task 7 — macOS acceptance and measurements

Files: none (manual check against `pnpm -C apps/desktop tauri dev` or the built app)

Interfaces: consumes Tasks 1-5.

Change: record the baseline WebKit RSS before the change if not already known, then run: `yes` for 60 s and Ctrl-C; `cat` of a file of at least 500 MB; heavy output while the window is minimized for 30 s, then restored. Record WebKit RSS, whether the UI stayed responsive, how long Ctrl-C took, and whether output after the minimize was intact.

Verify: WebKit RSS stays within a few hundred MB during `yes`; Ctrl-C stops `yes` within about 1 s; the UI keeps accepting input; output after the minimized period is complete or the valve behavior is recorded. `cannot verify` by automation because no WKWebView driver exists; these are manual observations.

Progress: done
Result: performed against a fresh `pnpm -C apps/desktop tauri dev` instance (stale pre-change app and daemon were restarted first; `daemon.log` confirms `gokod 0.1.2 listening`, and the new app spawned gokod as its child). Driven through `orca computer` accessibility input plus `ps`/`screencapture` for measurement — the equivalent of the manual check, run by the agent. Findings: (a) `yes` for ~90 s — `com.apple.WebKit.WebContent` RSS oscillated ~400-700 MB against a ~165 MB baseline (GC churn, not monotonic growth; the pre-gate failure held ~4.2 GB and a frozen UI), while `yes` ran at ~5% CPU — the producer visibly gated to xterm's parse rate; (b) Ctrl-C — `yes` was dead at the first 0.1 s poll after the key, `^C` rendered, and the prompt returned; the earlier visible round-trip including input dispatch measured under ~8 s worst case; (c) `cat` of a 512 MiB file completed — `CAT_DONE_0`, `time` reported 0.01 s user / 4.46 s system / 1:34 total, so virtually all wall time was render pacing; (d) window hidden 30 s — `yes` held at ~5% CPU, RSS *fell* to ~350 MB, restore showed the stream continuing with no gap and `echo MARKER_ALIVE` answered at once. UI stayed responsive throughout (AX tree live, input accepted, timer ticking). The 5 s stall valve did not fire in these runs — the view kept acking, just slowly; its behavior is covered by the bridge/daemon tests, not by this observation. Peak RSS (~695 MB during `cat`) is bounded but worth noting as xterm render/heap churn rather than output backlog; the backlog bound itself is provably small since the producer stalls at ~5% CPU.

## Files inspected, no change

- `crates/daemon/tests/lagged.rs` — the 256-chunk lag behavior that non-flow subscribers must keep.
- `apps/desktop/src-tauri/src/test_support.rs` — fake daemon helpers reused by Task 4 tests.
- `apps/desktop/src-tauri/src/terminal.rs::forward`, `::fresh` — dedupe on resubscribe stays as is.
- `docs/agent-devkit/changes/2026-11-04-stop-processes-on-exit/` — another open change that also adds a protocol request (`system.shutdown`); this change touches different variants of the same enum.
- `docs/llm/architecture/overview.md` — high-level process topology and terminal-stream summary; it does not make a claim about backpressure that this change would invalidate.

## Approval Gate

Status: approved

## Decision Log

- Replay accounting: all replayed snapshot bytes count against the 256 KiB window and `stream_output` paces them, instead of exempting the first snapshot. Reason: an exempt snapshot let up to 1 MiB enter the WebView on every resubscribe after a stall, so the backlog bound was not guaranteed. The acknowledged offset is session-scoped and monotonic so bytes the bridge already delivered cannot deadlock the pacing. Cost: a 1 MiB history load takes about four window round trips. A freshly mounted xterm facing a far-ahead offset may receive one unpaced replay burst of at most 1 MiB, followed by one live window and one 16 KiB read; stalls on that mount cannot repeat the burst. Chosen as the recommended option from review; gate stays `pending`.
- Stall handling: the bridge waits for the next webview ack before resubscribing after a flow `Lagged`, instead of resubscribing immediately. Reason: an immediate resubscribe restarts the 256 KiB window every 5 s while the WebView is hung, so its backlog grows without bound. Chosen as the recommended option from review; the alternative (immediate resubscribe) is simpler but not memory-bounded. Gate stays `pending` for joint approval.

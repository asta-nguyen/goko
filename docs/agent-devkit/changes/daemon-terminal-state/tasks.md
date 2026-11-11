# Daemon terminal state tasks

## Design

[Daemon-owned terminal state: first paint on attach](design.md)

## Global Constraints

- The raw ring buffer, kind-1 framing, flow control and `PROTOCOL_VERSION` are unchanged.
- Old daemons are never sent `paint`; they keep the raw replay path.
- Paint rendering happens under the `Shared` lock but never holds it across socket waits.
- No new dependency beyond `alacritty_terminal` 0.26.0 (already in the workspace) in the daemon.

## Impact map

Entry: `crates/daemon/src/pty.rs::PtyProcess::{spawn,resize,subscribe_flow}`, `server.rs::terminal_subscribe`/`stream_output`, `apps/rune/src/term.rs::Terminal::{feed,scroll}`, `panes.rs` attach and wheel handler.
Flow: PTY read → `Shared::publish` → ring buffer + model; subscribe(paint) → paint frames (`PAINT_SEQ`) → live chunks → Rune `Terminal::feed`.
State changes: daemon memory only (model per session); no disk, schema or settings change.
External effects: one optional field on `terminal.subscribe` and its reply; kind-1 frames with a reserved `seq`; PTY writes for wheel reports.
Verification: workspace build/fmt/clippy/test; round-trip and integration tests below; look at the fresh bundle.
Verified at: 373293e2b948a3cda8c54cf6e78e931da7aeb72d

## Tasks

### Task 1 — Daemon model and paint

Files: `crates/daemon/Cargo.toml`, `crates/daemon/src/lib.rs`, new `crates/daemon/src/terminal.rs` (`TerminalModel::{new, advance, resize, paint}`).
Acceptance: the round-trip test passes (cells incl. combining marks and wide chars, colors, flags, cursor, modes, history length, alternate screen, wrapped rows).
Verify: `cargo test -p daemon terminal`.

### Task 2 — Feed the model in the PTY lifetime and serve paint

Files: `crates/daemon/src/pty.rs`, `crates/daemon/src/server.rs`, `crates/protocol/src/{frame,message}.rs`, `crates/daemon/Cargo.toml` (0.1.3), `crates/daemon/tests/` integration test.
Acceptance: `terminal.subscribe {paint:true}` replies `paint:true`, sends paint frames then live chunks; a sentinel older than 1 MiB of later output is in the paint; `paint` absent keeps today's behavior.
Verify: `cargo test -p daemon -p protocol`.

### Task 3 — Rune consumes paint

Files: `apps/rune/src/daemon.rs` (`supports_paint`, `attach_output`, `SubscribeReply`), `apps/rune/src/term.rs::Terminal::feed`, `apps/rune/src/panes.rs` (attach call sites), tests.
Acceptance: a paint chunk rebuilds the grid without moving `next_seq`; the next live chunk is parsed whole; resubscribe replaces a stale grid.
Verify: `cargo test -p goko-rune term`.

### Task 4 — Mode-aware wheel

Files: `apps/rune/src/term.rs::Terminal::scroll`, `apps/rune/src/panes.rs` wheel handler, tests.
Acceptance: mouse mode → SGR/legacy wheel reports; alt-screen + alternate scroll → Up/Down (or `ESC O A/B` with app cursor); otherwise history; Shift always history.
Verify: `cargo test -p goko-rune term`.

### Task 4b — Answer the program's queries

Files: `apps/rune/src/term.rs` (`Replies` listener, `Terminal::take_replies`), `apps/rune/src/panes.rs::poll_terminals`.
Acceptance: after a first paint, `CSI 6n` and `CSI c` produce the cursor report and device attributes for the PTY; a stream without a paint produces none.
Verify: `cargo test -p goko-rune term`.

### Task 5 — Docs and final check

Files: `docs/llm/domains/rune-chrome.md` (RNC-terminal-attach, new RNC-terminal-wheel), `AGENTS.md` only if verification text changes.
Verify: the four workspace commands; the fresh `target/Goko.app` shown with `agent-desktop` on its own window.

## Results

- Baseline before edits: `cargo test --workspace` all suites ok.
- Task 1: `cargo test -p daemon terminal` — 4 passed (history/screen/cursor/modes round trip, alternate screen, split sequence, resize).
- Task 2: `cargo test -p daemon -p protocol` ok; `tests/paint.rs` — a sentinel older than the 1 MiB raw buffer is absent from the raw replay and present in the paint, and the socket test sees `paint:true`, `PAINT_SEQ` frames, then live output at `next_seq`. The existing `flow.rs` test `dropping_the_receiver_releases_the_reader` waited on `<= WINDOW` while the reader pauses at `>= WINDOW`; the model made reads regular enough to land exactly on it, so it now waits on `< WINDOW`.
- Tasks 3–4: `cargo test -p goko-rune` 696 passed (paint feed, wheel by mode).
- Task 4b: `queries_are_answered_on_a_paint_stream_and_not_on_a_replay` — `CSI 6n` and `CSI c` answered after a paint (`\x1b[1;4R`, `\x1b[?6c`), nothing on a raw replay.
- Task 5: wiki updated (`RNC-terminal-attach`, new `RNC-terminal-wheel`).
- Whole workspace: `cargo build`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` all pass.
- Latest full run: `cargo test --workspace` 23 suites ok, `clippy -D warnings` and `fmt --check` clean.
- Not done: the fresh `target/Goko.app` has not been looked at in a real window, and `review-and-verify` has not run (so the change stays open).

## Baseline

Before any edit: `cargo test --workspace` passes (all suites ok; Rune included).

## Approval Gate

Status: approved — the user asked to implement and to revise the plan if needed, after the reduced scope (daemon model + first paint + wheel by mode, glyph rendering later) was proposed in chat.

## Decision Log

- Use a first-paint model (an ANSI rebuild sent before the live stream) instead of a cell-snapshot protocol; see [research.md](research.md).
- Display offset stays GUI-local (no shared session offset).
- Alternate-screen primary buffer and scroll region are not restored; marked with `ponytail:` comments.

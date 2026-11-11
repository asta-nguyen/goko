# Source evidence and Zed comparison

Planning evidence, not implementation or a claim that either app was tested by
running Zed. Goko baseline: `373293e2b948a3cda8c54cf6e78e931da7aeb72d`.
Zed default branch pinned during research to
`82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9` (2026-11-11).

## Observed Goko incident

Read the reported session's daemon snapshot through an authenticated temporary
non-flow subscription. The script printed metadata and escape counts only,
never the token or conversation text, and closed its connection afterwards.

- Goko session: `a0e47851-ffa7-4be2-a1f7-ce830f2bcb63`, child PID 97556.
- `replay_from = 3978981`, `next_seq = 5027557`.
- Retained bytes: 1,048,576; earlier output was already evicted.
- Retained LF count: zero; no alternate-screen enter/leave sequence remained
  within that tail. The original buffer mode cannot be inferred from absence
  of its prefix. These observations do not prove Codex conversation data was lost.
- The release launch script replaced the GUI process. Its local emulator state
  could not survive that replacement; the daemon and child process did survive.

Source navigation:
- `crates/daemon/src/buffer.rs::BUFFER_CAPACITY`, `RingBuffer::append` — one MiB
  raw replay, byte-based eviction independent of ANSI boundaries.
- `apps/rune/src/term.rs::Terminal::new` — empty local grid and processor;
  `Terminal::scroll` — grid display offset only.
- `apps/rune/src/panes.rs::sync_terminals` — open tabs retain local parsers even
  while hidden, but a new GUI process still reconstructs from replay.
- `crates/daemon/src/pty.rs::spawn_reader`, `Shared::publish`, `PtyProcess::resize`
  — separate read-publication and master resize locking today.
- `crates/daemon/src/server.rs::terminal_subscribe`, `stream_output` — subscribe
  response precedes raw replay, preserving the current byte offset contract.
- `apps/rune/src/daemon.rs::supports_flow_control`, supervisor and bootstrap —
  an old running daemon is reused; bundling a new daemon does not upgrade it.

## What Zed does

| Concern | Source-established behavior |
|---|---|
| Local terminal ownership | `Terminal` owns an `Arc<AlacrittyTermLock>` and PTY resources in the editor process; its PTY event loop shares that same lock/model |
| Scrollback | Normal terminals default to 10,000 lines and cap configuration at 100,000; task terminals use the larger cap |
| Workspace persistence | Saves working directory and custom title; deserialization creates a new terminal shell, not an old terminal-state restore |
| Wheel | Mouse-reporting mode sends mouse reports; alternate screen plus alternate-scroll sends key sequences; otherwise adjusts terminal history |
| Resource lifetime | Dropping a local terminal releases its PTY resources; this path is not Goko's independent persistent terminal daemon |

Primary source links, all pinned to the same commit:
- [Terminal ownership](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal/src/terminal.rs#L1522-L1564).
- [PTY/model setup](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal/src/alacritty.rs#L191-L235).
- [History policy](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal/src/terminal.rs#L1211-L1221).
- [Mouse and alternate-screen wheel routing](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal/src/terminal.rs#L2892-L2925).
- [Persist/restore implementation](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal_view/src/terminal_view.rs#L2015-L2107).
- [Persisted database columns](https://github.com/zed-industries/zed/blob/82f2558e17f6459f2cc8cb4a9fbe05e36520cdd9/crates/terminal_view/src/persistence.rs#L410-L453).

This comparison is limited to the local terminal path and the workspace
persistence implementation above. It does not claim all Zed remote/SSH/agent
terminal implementations have the same lifetime. Goko should reuse the
model/view separation and mode-aware input principles, while placing the
canonical model in its already-independent daemon to satisfy its stronger
restart requirement. Do not copy a promise of full-session persistence from Zed.

## Pinned Alacritty feasibility

Inspected installed `alacritty_terminal` 0.26.0 source against Cargo.lock:
`Term::new`, `resize`, `mode`, `grid`, `scroll_display`, `renderable_content`,
`Processor::advance`, `Grid::display_iter`, `Cell` flags/zero-width text and
the event listener API. Public APIs suffice to retain the canonical emulator
and export a viewport. `Grid` has optional serde, but its cursor and saved
cursor are skipped; `Term` also contains private inactive-grid/parser-related
state. An image of a serialized active grid plus resumed raw bytes is not a
complete restore protocol. The design therefore never restores an upstream
`Term` on the client and needs no upstream fork or private-field access.

## A reference terminal host (second comparison, decides the approach)

A published terminal app keeps its terminals in a separate daemon that holds an
emulator per terminal, with the window as a viewer. On attach the daemon sends a
**first paint** (current screen, optional history, title and mode toggles rendered
as VT bytes), then the program's raw output; the viewer parses both with its own
emulator. Flow control is a credit window (256 KiB) and a viewer more than 4 MiB
behind is repainted instead of backlogged. The viewer answers the program's
terminal queries (cursor position, device attributes, size reports) from its own
emulator and writes the answers back as automatic input. Reboot or daemon crash
gives a fresh shell, not the old program — the same limit Goko keeps.

Consequence: Goko uses this model (daemon `Term` + ANSI first paint over the
existing raw stream) instead of a cell-snapshot protocol. Differences: Alacritty
has no formatter, so Goko writes its own paint. Unanswered queries were a lead for
Codex's odd layout: Rune's parser dropped every reply, so a program that asks where
the cursor is got nothing. Rune now answers them (see the design).

What that app's `paint(history)` does that Goko's does not: it takes a `history`
row cap, restores the hidden primary screen under an alternate-screen program
through a snapshot copy, and replays scroll region, tab stops, charsets and kitty
keyboard flags through its formatter. Goko paints all retained history, only the
active screen, and skips those extras (`ponytail:` in `terminal.rs`); Alacritty
exposes no inactive grid or formatter. Goko took its synchronized-update handling
from this reading.

It also places every glyph at its own cell column instead of trusting the font's
advance. Goko draws a row as one text run with cell backgrounds at `col * cell_w`,
so a glyph whose advance is not one cell drifts the rest of the row; this is the
likely cause of the misaligned cell backgrounds and is not fixed yet.

# Daemon-owned terminal state: first paint on attach

Replaces the earlier snapshot-stream draft (viewport DTOs, JSON/base64 parts,
shared scroll offset). Evidence and the reference comparison are in
[research.md](research.md).

## Goal

A GUI that attaches or re-attaches to a surviving `gokod` session sees the
session's current screen and retained scrollback, not a reconstruction from the
last 1 MiB of raw output. The terminal wheel also behaves like a terminal's:
mouse reports, alternate-screen arrow keys, or history, by the program's mode.

## Approach

The daemon keeps one `alacritty_terminal` `Term` + VTE `Processor` per PTY
session, fed every output chunk from creation, with or without a GUI. A
subscriber that asks for it gets a **first paint** — an ANSI byte string that
rebuilds the screen — and then the existing live raw stream. The GUI keeps its
own parser and renderer unchanged; it just starts from the paint.

## Behavior

### B1 — Model in the daemon

`PtyProcess` owns a model created at the spawn size. `Shared::publish` parses
each chunk into it under the same lock that appends to the ring buffer, so the
model and the raw offset move together. `resize` resizes the model after the PTY
resize succeeds. Primary history is Alacritty's default 10,000 rows; the model
is dropped with the session. Query replies stay dropped (`VoidListener`), as in
the GUI today.

### B2 — First paint

`terminal.subscribe` gains `paint: bool` (default false, omitted when false).
When true the daemon, in the same critical section that registers the
subscriber, renders the paint and answers `{replay_from: next_seq, next_seq,
paint: true}`; the raw replay is skipped. The paint is sent as kind-1 frames
whose `seq` is `protocol::frame::PAINT_SEQ` (`u64::MAX`, "not a stream
offset"), in pieces under the frame limit, before any live chunk. Live chunks
then continue at `next_seq` unchanged, so ack/flow control is untouched: paint
bytes are outside flow accounting, and the acknowledged offset starts at
`next_seq`.

The paint is, in order: `ESC c` (reset), the primary history rows then the
visible rows (rows that wrap are written without a line break; SGR is emitted
only on change; wide characters once, combining marks kept with their base;
trailing default blanks trimmed), the cursor position and shape, and the modes
below. In the alternate screen it paints `ESC[?1049h` and the alternate grid
only.

Modes restored: application cursor, application keypad, bracketed paste, mouse
report/drag/motion, SGR and UTF-8 mouse encoding, focus reporting, alternate
scroll, line wrap, origin, cursor visibility.

Before painting, the model flushes any synchronized update (CSI ?2026h) the
parser is still holding back, so a paint taken mid-frame shows that frame so far.

Known limits (not silently dropped; see `ponytail:` comments): the primary
screen underneath a running alternate-screen program, the scroll region, tab
stops, charsets and hyperlinks are not restored — the program's next redraw
sets the region and a program that left the alternate screen redraws.

### B3 — Rune consumption

`Terminal::feed` treats a chunk with `seq == PAINT_SEQ` as paint: it parses it
into the grid without touching `next_seq`. The first live chunk is "ahead of
`next_seq`" and is parsed whole, which the feed already supports. Rune asks for
`paint` when the daemon version is at least 0.1.3 and otherwise attaches as
today (raw replay). No new request is sent to an old daemon.

### B3b — Answering queries (Rune)

Rune's parser used to drop every reply it generated, so a program that asked
where the cursor is (CSI 6n) or what the terminal is (CSI c) got no answer.
Rune now collects the parser's replies and writes them to the PTY through
`write_terminal`. Only on a stream that started with a first paint: a raw replay
from an old daemon replays queries the program was already answered, and
answering them again would type the answers into it. Colour and text-area-size
queries are not answered yet (`ponytail:` in `term.rs`).

### B4 — Wheel routing (Rune)

`Terminal::scroll` becomes mode-aware. Without Shift: if the program requested
mouse reporting, send wheel reports (SGR or legacy encoding, direction, cell
position, bounds); else if on the alternate screen with alternate-scroll, send
Up/Down (application-cursor aware); else scroll the history as today. Shift
always scrolls history. The bytes go through the existing `write_terminal`.

## Compatibility

`PROTOCOL_VERSION`, request names, raw framing and flow control are unchanged;
the daemon package becomes 0.1.3. An old daemon keeps working with the old
replay path. A running old daemon is never restarted for the user; the old
session cannot be recovered from its truncated ring.

## Out of scope

Disk persistence or process revival, a viewport-snapshot protocol, per-viewer
scroll offsets, answering queries from the daemon (Rune answers them), the
glyph-advance (wide and combining character) rendering issue, and changing the
emulator crate.

## Verification

Round-trip test in the daemon: bytes → model A → paint → model B, compared cell
by cell (characters, combining marks, colors, flags), cursor, modes and history
length, including alternate screen and wrapped rows. Real-daemon integration
test: a sentinel line older than 1 MiB of later output is still in the paint of
a fresh subscribe while the child stays alive. Rune tests: paint feeds the grid
and live bytes continue; wheel routing per mode. Then `cargo build --workspace`,
`cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
`cargo test --workspace`, and a look at the fresh `target/Goko.app` with
`agent-desktop` scoped to that window.

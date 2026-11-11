# Delta

Requirements for [design](design.md) and [tasks](tasks.md); merged into the wiki (task 5).

## MODIFIED Requirements

### RNC-terminal-attach

Previously: Rune owns its parser/grid and reattaches from raw retained output. The baseline's switch-session unsubscribe scenario is also stale relative to current hidden-tab attachment retention; the replacement specifies the intended canonical and legacy paths explicitly.

Baseline:

````md
### RNC-terminal-attach

The system SHALL attach the Terminal view to the selected daemon session:
`terminal.subscribe` replays retained output then streams live kind-1 frames
into the alacritty `Term` grid rendered as styled runs; keyboard input
encodes to `terminal.write`, viewport changes send `terminal.resize`,
`terminal.lagged` or a session switch resubscribes, `terminal.unsubscribe`
runs on detach, and an exited session keeps its final grid with input
suppressed.

#### Scenario: attach and interact

- GIVEN a running session selected in the Terminal view
- WHEN output frames arrive
- THEN the grid renders them; typing sends bytes the session receives (echo
  returns through the same path)

#### Scenario: lagged subscriber recovers

- GIVEN the terminal subscriber fell behind
- WHEN the daemon sends `terminal.lagged`
- THEN the client unsubscribes and resubscribes, replaying from the retained
  buffer

#### Scenario: switch sessions

- GIVEN session A attached
- WHEN session B is selected
- THEN A is unsubscribed, B subscribes, and B's retained+live output renders

Evidence: `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/main.rs`, `crates/protocol/src/frame.rs`, `crates/protocol/src/message.rs`
````

Replacement:

````md
### RNC-terminal-attach

The system SHALL keep each session's terminal emulator state in the daemon and, when the daemon supports it, start an attaching Terminal view from a first paint (an ANSI rebuild of the retained scrollback, the current screen, cursor and terminal modes) followed by the live kind-1 stream; a daemon without that support is attached through the retained raw replay.

#### Scenario: GUI restart with retained history

- GIVEN a daemon kept a session while its output exceeded the raw replay buffer
- WHEN the GUI attaches again
- THEN the grid shows the current screen and the retained scrollback, and live output continues from the paint without a gap or duplicate

#### Scenario: no viewers

- GIVEN a session has no subscribers
- WHEN it writes output
- THEN the daemon's emulator keeps consuming it, so a later attach paints the current state

#### Scenario: compatible legacy daemon

- GIVEN the daemon predates first paint
- WHEN Rune opens a session
- THEN it attaches with the raw replay and sends no unsupported request
````

## ADDED Requirements

### RNC-terminal-wheel

Page: `docs/llm/domains/rune-chrome.md`
Search: checked all RNC pages and open deltas; no existing requirement specifies mode-aware wheel reporting or alternate-screen scroll keys. Existing terminal-splits behavior remains unchanged.

Requirement:

````md
### RNC-terminal-wheel

The system SHALL route terminal wheel input according to the program's mouse and alternate-screen modes, sending PTY wheel reports or alternate-scroll keys when requested and otherwise scrolling terminal history, with Shift selecting history instead of application reporting.

#### Scenario: primary history

- GIVEN primary-screen output has retained history and no mouse capture applies
- WHEN the user wheels upward
- THEN the view reveals older retained rows

#### Scenario: mouse reporting

- GIVEN the application requested terminal mouse reports
- WHEN the user wheels without Shift
- THEN the PTY receives reports with the correct direction, modifiers, coordinate encoding and bounds

#### Scenario: alternate scrolling

- GIVEN alternate screen and alternate-scroll are enabled without mouse capture
- WHEN the user wheels without Shift
- THEN the PTY receives correctly encoded Up/Down input rather than fabricated alternate-screen history

#### Scenario: explicit history override

- GIVEN a terminal application captures mouse or alternate scrolling
- WHEN the user wheels with Shift
- THEN the operation targets emulator history and emits no application wheel report; alternate-screen history may be empty

Evidence: `apps/rune/src/term.rs`, `apps/rune/src/panes.rs`
````

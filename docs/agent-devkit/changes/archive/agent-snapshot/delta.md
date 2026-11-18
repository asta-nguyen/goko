# Delta

Merged into docs/llm/: 2026-11-16

Approved: 2026-11-16 — snapshot copies the selection or whole file to the clipboard, and a separate action sends it to the focused terminal

## ADDED Requirements

### RNE-agent-snapshot

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers copying a snapshot to the clipboard or the terminal.

Requirement:

````md
### RNE-agent-snapshot

The system SHALL let the user copy a snapshot of the Files code surface — the selected lines expanded to the whole lines they touch, or the whole open file when there is no selection — carrying the checkout-relative path, its 1-based line range, the file's language, an `unsaved` marker when the buffer differs from disk, and the code in a fence longer than any backtick run in it, to the clipboard, and send that same snapshot into the focused terminal session as a bracketed paste when the program asked for one, without submitting it.

#### Scenario: Copy a selection

- GIVEN an editable file is open in Files with part of lines 120–148 selected
- WHEN the user presses ⌘⇧C
- THEN the clipboard holds `src/editor/code.rs:120-148 (rust)` followed by a rust-fenced block of those whole lines, and the "Snapshot copied to clipboard" notice shows

#### Scenario: Copy the whole file

- GIVEN an editable file is open with no selection
- WHEN the user presses ⌘⇧C
- THEN the clipboard holds the whole file under its `path:1-N` header

#### Scenario: Unsaved buffer

- GIVEN the open buffer differs from the file on disk
- WHEN a snapshot is copied or sent
- THEN it carries the buffer's text and its header marks the code unsaved

#### Scenario: Send to the terminal

- GIVEN the active tab has a live focused terminal session
- WHEN the user presses ⌘⇧Enter or presses the code surface's Send chip
- THEN the snapshot reaches that session as a bracketed paste when the program asked for one, no Enter is sent, and the notice names the session

#### Scenario: No live session

- GIVEN the active tab has no live focused session
- WHEN the user sends a snapshot
- THEN a notice says so and no bytes are written

#### Scenario: Fence collision

- GIVEN the selected code contains a run of backticks
- WHEN the snapshot is built
- THEN its fence is longer than the longest run in the code

#### Scenario: Not an editable file

- GIVEN the open document is binary, non-UTF-8, truncated, or read-only
- WHEN the user presses the snapshot chord
- THEN no snapshot is copied and a notice explains why

Evidence: `apps/rune/src/editor/snapshot.rs`, `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/files.rs`
````

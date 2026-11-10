# Delta

Approved: user approved the chat design in conversation (bounded lane)

## MODIFIED Requirements

### RNE-code-surface

Previously: the surface offers `⌘S`/`⌘F`/`⌘G`/`⇧⌘G`/`Escape` chords only; Tab and Enter perform no code-aware indentation.

Baseline:

````md
### RNE-code-surface

The system SHALL edit the open document in a rune-kit native text surface with a line-number gutter, offering `⌘S` save, `⌘F` find with match count and `⌘G`/`⇧⌘G` match navigation, `Escape` to close the find bar, and read-only selectable rendering for files that cannot be edited.

#### Scenario: Edit and save a file

- GIVEN an editable UTF-8 file is open
- WHEN the user types and presses `⌘S`
- THEN the save enqueues through the write lane and the document's original text updates only on success

#### Scenario: Find in document

- GIVEN a document is open
- WHEN the user presses `⌘F`, types a query, and presses `⌘G`
- THEN the find bar shows the match count and steps through matches; `Escape` closes it

#### Scenario: Read-only file

- GIVEN a binary, non-UTF-8, truncated, oversized, or read-only file is open
- WHEN the document renders
- THEN it shows the read-only surface and no edit or save path is offered

Evidence: `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/find.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/navigation.rs`
````

Replacement:

````md
### RNE-code-surface

The system SHALL edit the open document in a rune-kit native text surface with a line-number gutter, offering `⌘S` save, `⌘F` find with match count and `⌘G`/`⇧⌘G` match navigation, `Escape` to close the find bar, `Tab`/`Shift+Tab` line indentation, `Enter` auto-indent, and read-only selectable rendering for files that cannot be edited.

#### Scenario: Edit and save a file

- GIVEN an editable UTF-8 file is open
- WHEN the user types and presses `⌘S`
- THEN the save enqueues through the write lane and the document's original text updates only on success

#### Scenario: Find in document

- GIVEN a document is open
- WHEN the user presses `⌘F`, types a query, and presses `⌘G`
- THEN the find bar shows the match count and steps through matches; `Escape` closes it

#### Scenario: Tab indents the caret line or selection

- GIVEN an editable document and the resolved indent unit
- WHEN `Tab` is pressed with no selection, or `Shift+Tab` with a multi-line selection
- THEN the caret line gains the indent unit, or every touched line's leading whitespace is indented/outdented by one unit as a single undoable edit

#### Scenario: Enter keeps the line's indent

- GIVEN an editable document whose current line has leading whitespace
- WHEN `Enter` is pressed
- THEN the new line starts with that whitespace, plus one indent unit when the line ends with an opening brace, bracket, or colon

#### Scenario: Read-only file

- GIVEN a binary, non-UTF-8, truncated, oversized, or read-only file is open
- WHEN the document renders
- THEN it shows the read-only surface and no edit or save path is offered

Evidence: `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/find.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/navigation.rs`
````

## ADDED Requirements

### RNE-editor-indent

The system SHALL resolve the indent unit per document — manual override (`auto`, `2`, `4`, `8`, or `tab`) persisted through the shared settings store, else content sniffing of the dominant leading-whitespace unit, else a per-language default, else 4 spaces — show it on a compact status chip that cycles the override on press, and draw subtle indent guide ticks inside the code surface at each multiple of the resolved unit a line's leading-whitespace visual column reaches, computed for the whole document up to a bounded line count, merged into a few draw segments, and mounted inside the scroll content so it tracks scrolling natively; files already containing tab characters render them at the framework's fixed tab stops, and the resolved unit governs inserted whitespace and guides only.

#### Scenario: guides follow indentation

- GIVEN a document whose lines are indented with the resolved unit
- WHEN the editor renders and scrolls
- THEN each visible line shows guide ticks at its indent multiples, aligned to the monospace grid, without shifting text layout

#### Scenario: override persists

- GIVEN the status chip shows the resolved unit
- WHEN the user presses it to cycle to `4` and restarts the app
- THEN new documents default to the `4` override until changed back to `auto`

#### Scenario: auto detection

- GIVEN no manual override and a document whose indented lines dominantly use two spaces
- WHEN the editor resolves the indent unit
- THEN Tab inserts two spaces and guides draw every two columns

Evidence: `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/indent.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/store.rs`

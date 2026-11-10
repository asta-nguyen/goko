# Delta

Merged into docs/llm/: 2026-11-10

Approved: 2026-11-10 — accept the highlighting cost and support full-file highlighting through rune-highlights

## MODIFIED Requirements

### RNE-syntax-highlight

Previously: covered only the first 32 KiB of source (cut back to a line end) and at most 8192 ranges, with a visible partial-highlighting notice.

Baseline:

````md
### RNE-syntax-highlight

The system SHALL colour supported languages with rune-kit's syntax scanners, covering only the first 32 KiB of source (cut back to a line end) and at most 8192 ranges while showing a visible partial-highlighting notice, keeping the complete file editable up to the 1 MiB cap, rendering unknown languages as plain text, and computing highlights off the UI thread scoped to document revision so stale results never render.

#### Scenario: Large file partially highlighted

- GIVEN a source file larger than 32 KiB or producing more than 8192 ranges
- WHEN it opens
- THEN only the budgeted prefix is coloured, a partial-highlighting notice shows, and the whole file remains editable

#### Scenario: Stale highlight dropped

- GIVEN a highlight job was issued for revision N
- WHEN the user edits before the reply lands
- THEN the reply's revision no longer matches and it is dropped; a newer revision re-arms the debounce

#### Scenario: Unknown language

- GIVEN a file whose extension maps to no scanner
- WHEN it opens
- THEN it renders as plain text and no highlight job is enqueued

Evidence: `apps/rune/src/editor/syntax.rs`, `apps/rune/src/editor/highlight.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`
````

Replacement:

````md
### RNE-syntax-highlight

The system SHALL colour supported languages with rune-highlights lexers across the complete document up to the 1 MiB file cap — no byte-prefix truncation — while bounding coloured ranges at 131072 with a visible notice when that budget is reached, keeping the complete file editable, rendering unknown languages as plain text, and computing highlights off the UI thread scoped to document revision so stale results never render.

#### Scenario: Large file fully highlighted

- GIVEN a source file larger than 32 KiB in a supported language
- WHEN its highlight result applies
- THEN every line through the end of the file is coloured, no partial-highlighting notice shows, and the whole file remains editable

#### Scenario: Range budget reached

- GIVEN a document whose tokens exceed the 131072-range budget
- WHEN its highlight result applies
- THEN colouring stops at the budget, a visible capped notice shows, and the whole file remains editable

#### Scenario: Stale highlight dropped

- GIVEN a highlight job was issued for revision N
- WHEN the user edits before the reply lands
- THEN the reply's revision no longer matches and it is dropped; a newer revision re-arms the debounce

#### Scenario: Unknown language

- GIVEN a file whose extension maps to no lexer
- WHEN it opens
- THEN it renders as plain text and no highlight job is enqueued

Evidence: `apps/rune/src/editor/syntax.rs`, `apps/rune/src/editor/highlight.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/Cargo.toml`
````

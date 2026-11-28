# Delta

Merged into docs/llm/: 2026-11-28

Approved: 2026-11-28 — Replace the fixed blame column beside the code with one annotation drawn inline at the end of the caret's line, hidden while the draft is dirty, toggle and click-through unchanged.

## MODIFIED Requirements

### RNE-inline-blame

Previously: the annotation lived in a fixed column beside the code, blanked while the draft was dirty but kept mounted so the code did not reflow.

Baseline:

````md
### RNE-inline-blame

The system SHALL show, next to the open file's code, an annotation on the caret's line naming the author who last touched it and how long ago the commit landed, read through `git blame` on the worker lane — the annotation blanked (the column stays mounted) while the buffer holds unsaved edits, re-read after the save lands, and toggled on and off from the editor footer with the choice persisted under `goko.editor.blame`. Pressing the annotation of a committed line opens that commit in History: the section switches, and History pages on until the commit is loaded, then selects and reveals it (it gives up once the history runs out).

#### Scenario: Annotations land beside the code

- GIVEN a tracked file is open in a checkout
- WHEN the blame read lands
- THEN the column shows the caret line's author name and a compact age, following the caret, and a line changed in the working tree but never committed reads "uncommitted"

#### Scenario: A dirty draft blanks the annotation

- GIVEN the annotation is shown
- WHEN the user edits the buffer
- THEN the annotation goes blank because it describes the file on disk, the column keeps its width so the code does not reflow, and fresh annotations return after the save lands

#### Scenario: The toggle persists

- GIVEN the user presses the blame chip in the editor footer
- WHEN the app restarts
- THEN the column comes back in the state the user left it, stored under `goko.editor.blame`

#### Scenario: A file Git cannot blame

- GIVEN the open file is untracked or otherwise rejected by Git
- WHEN the blame read fails
- THEN the editor shows the error and no annotation is invented

Evidence: `apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/files.rs`, `crates/app-core/src/git_view.rs`
````

Replacement:

````md
### RNE-inline-blame

The system SHALL draw, inline at the end of the caret's line in the open file's code, an annotation naming the author who last touched that line and how long ago the commit landed, read through `git blame` on the worker lane — the annotation hidden while the buffer holds unsaved edits, re-read after the save lands, and toggled on and off from the editor footer with the choice persisted under `goko.editor.blame`. Pressing the annotation of a committed line opens that commit in History: the section switches, and History pages on until the commit is loaded, then selects and reveals it (it gives up once the history runs out).

#### Scenario: Annotations land at the end of the caret's line

- GIVEN a tracked file is open in a checkout
- WHEN the blame read lands
- THEN the annotation sits after the last character of the caret's line with its author's name and a compact age, following the caret as it moves, and a line changed in the working tree but never committed reads "uncommitted"

#### Scenario: A dirty draft hides the annotation

- GIVEN the annotation is shown
- WHEN the user edits the buffer
- THEN the annotation disappears because it describes the file on disk, the code does not reflow, and fresh annotations return after the save lands

#### Scenario: The toggle persists

- GIVEN the user presses the blame chip in the editor footer
- WHEN the app restarts
- THEN the annotation comes back in the state the user left it, stored under `goko.editor.blame`

#### Scenario: A file Git cannot blame

- GIVEN the open file is untracked or otherwise rejected by Git
- WHEN the blame read fails
- THEN the editor shows the error and no annotation is invented

Evidence: `apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/files.rs`, `crates/app-core/src/git_view.rs`
````

# Delta

Approved: user approved the chat design in conversation (bounded lane) — Rune only, inline per-line blame with a toggle, hidden while the draft is dirty and re-read after the save; name + time only, no click-through.

## ADDED Requirements

### RNE-inline-blame

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers attributing each line of the open file to the author who last touched it.

Requirement:

````md
### RNE-inline-blame

The system SHALL show, next to the open file's code, one annotation per line naming the author who last touched that line and how long ago the commit landed, read through `git blame` on the worker lane; the column SHALL be hidden while the buffer holds unsaved edits, re-read after the save lands, and toggled on and off from the editor footer with the choice persisted under `goko.editor.blame`.

#### Scenario: Annotations land beside the code

- GIVEN a tracked file is open in a checkout
- WHEN the blame read lands
- THEN each code line is followed by a column entry with its author's name and a compact age, and a line changed in the working tree but never committed reads "uncommitted"

#### Scenario: A dirty draft hides the column

- GIVEN the annotations are shown
- WHEN the user edits the buffer
- THEN the column is hidden because the annotations describe the file on disk, and it returns with fresh annotations after the save lands

#### Scenario: The toggle persists

- GIVEN the user presses the blame chip in the editor footer
- WHEN the app restarts
- THEN the column comes back in the state the user left it, stored under `goko.editor.blame`

#### Scenario: A file Git cannot blame

- GIVEN the open file is untracked or otherwise rejected by Git
- WHEN the blame read fails
- THEN the editor shows the error and no annotation is invented

Evidence: `apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/files.rs`, `crates/app-core/src/git_view.rs`
````

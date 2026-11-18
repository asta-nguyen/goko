# Delta

Merged into docs/llm/: 2026-11-18

## ADDED Requirements

### RNE-editor-tabs
Page: `docs/llm/domains/rune-editor.md`
Search: checked every RNE requirement; `RNE-file-editing` and `RNE-draft-guard` describe one open document, none describes several open files, tab switching or closing.
Requirement:
````md
### RNE-editor-tabs

The system SHALL keep the files opened from the current checkout as tabs over the editor pane — each holding its own unsaved buffer, undo history and caret, opened by every existing open-file route without a dialog, switched by click, by `⌘⌥→`/`⌘⌥←` in tab order (`Ctrl+PageDown`/`Ctrl+PageUp` off macOS) or by `⌃Tab` to the most recently used tab, closed by its × button or `⌘W`, showing the file name (with its directory when names collide) and a dirty marker, and all closed together on checkout change.

#### Scenario: Edit, switch away and back

- GIVEN a file is open with unsaved edits
- WHEN the user opens a second file and then clicks the first tab
- THEN the first file shows its unsaved text, caret and undo history

#### Scenario: Close a dirty tab

- GIVEN the active tab has unsaved edits
- WHEN the user presses its × button
- THEN the Save/Discard/Cancel guard names that file, and Cancel keeps the tab open

#### Scenario: Back to the most recent tab

- GIVEN three tabs are open and the user moved from the second to the third
- WHEN the user presses `⌃Tab` twice
- THEN the second tab activates and then the third again

Evidence: `apps/rune/src/editor/tabs.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/state.rs`
````

## MODIFIED Requirements

### RNE-draft-guard
Previously: opening another file was guarded, and the guard asked about one open draft.
Baseline:
````md
### RNE-draft-guard

The system SHALL route every navigation that would leave a dirty draft — opening another file, switching checkout or project, switching section or view, closing the window, and quitting — through a Save/Discard/Cancel guard, resuming the pending navigation only after a successful save, reverting on Discard, and preserving both editor and draft on Cancel or failed save.

#### Scenario: Guard on view switch

- GIVEN the open file has unsaved changes and `view` is `Editor`
- WHEN the user presses the Terminal view item and chooses Cancel
- THEN `view` stays `Editor` with the draft intact; choosing Save enqueues the write and switches only when it lands

#### Scenario: Guard on window close and quit

- GIVEN a dirty draft
- WHEN the user closes the window or quits the app
- THEN the close/quit is cancelled pending the guard on every platform for window close, and on macOS an OS-level quit is vetoed via the `applicationShouldTerminate:` swizzle; on other platforms OS-level quit remains an honest gap

#### Scenario: Discard reverts

- GIVEN a dirty draft and a pending navigation
- WHEN the user chooses Discard
- THEN the buffer reverts to the loaded original and the pending navigation proceeds

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/quit_guard.rs`, `apps/rune/src/main.rs`
````
Replacement:
````md
### RNE-draft-guard

The system SHALL route every navigation that would leave a dirty draft — closing a dirty tab, switching checkout or project, switching section or view, closing the window, and quitting — through one Save/Discard/Cancel guard that lists every dirty tab (Save All / Discard All / Cancel when there is more than one), resuming the pending navigation only after every listed tab was saved or discarded, reverting on Discard, and preserving every tab and draft on Cancel or failed save; opening or switching to another file in the same checkout leaves a dirty draft in its tab and never prompts.

#### Scenario: Guard on view switch

- GIVEN the open file has unsaved changes and `view` is `Editor`
- WHEN the user presses the Terminal view item and chooses Cancel
- THEN `view` stays `Editor` with the draft intact; choosing Save enqueues the write and switches only when it lands

#### Scenario: Guard on window close and quit

- GIVEN a dirty draft
- WHEN the user closes the window or quits the app
- THEN the close/quit is cancelled pending the guard on every platform for window close, and on macOS an OS-level quit is vetoed via the `applicationShouldTerminate:` swizzle; on other platforms OS-level quit remains an honest gap

#### Scenario: Opening another file keeps the draft

- GIVEN the active tab has unsaved edits
- WHEN the user opens another file in the same checkout
- THEN no dialog shows, the edited tab stays open with its draft and the new file opens as the active tab

#### Scenario: Several dirty tabs

- GIVEN two tabs have unsaved edits
- WHEN the user switches checkout
- THEN one dialog lists both files; Save All saves them in tab order and then switches, and if the second save fails the checkout does not change and the failing tab stays active with its error

#### Scenario: Discard reverts

- GIVEN a dirty draft and a pending navigation
- WHEN the user chooses Discard
- THEN the buffer reverts to the loaded original and the pending navigation proceeds

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/tabs.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/quit_guard.rs`, `apps/rune/src/main.rs`
````

### RNE-keyboard
Previously: the global chords did not include tab close or tab switching.
Baseline:
````md
### RNE-keyboard

The system SHALL handle the editor's global chords — `⌘S` save, contextual `⌘F` find and `⇧⌘F` content search, `⌘P` quick-open — from anywhere in the Editor view, offering `1`–`4` section switching (Files/Changes/History/Stashes; Servers has no digit) plus `j`/`k` highlight-cursor movement with `Enter` activation only while a checkout is selected, no editable field holds focus or an IME composition, and no modal surface owns input.

#### Scenario: Chord from anywhere

- GIVEN the Editor view is active and a document or list has focus
- WHEN the user presses `⌘S`, `⌘F`, `⇧⌘F`, or `⌘P`
- THEN save, find, content-search, or the quick-open palette triggers even while a field is focused

#### Scenario: Guarded plain keys

- GIVEN no checkout is selected, an editable field is focused, an IME composition is active, or a modal (draft guard, picker, context menu, quick-open) owns input
- WHEN the user presses `1`–`4`, `j`, `k`, or `Enter`
- THEN the shortcut does not fire; with a clean state `j`/`k` move only the highlight cursor and `Enter` activates through the row's existing handler

Evidence: `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/quick_open.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/main.rs`
````
Replacement:
````md
### RNE-keyboard

The system SHALL handle the editor's global chords — `⌘S` save, contextual `⌘F` find and `⇧⌘F` content search, `⌘P` quick-open, `⌘W` close the active tab, `⌘⌥→`/`⌘⌥←` next and previous tab (`Ctrl+PageDown`/`Ctrl+PageUp` off macOS), `⌃Tab` the most recently used tab — from anywhere in the Editor view, offering `1`–`4` section switching (Files/Changes/History/Stashes; Servers has no digit) plus `j`/`k` highlight-cursor movement with `Enter` activation only while a checkout is selected, no editable field holds focus or an IME composition, and no modal surface owns input.

#### Scenario: Chord from anywhere

- GIVEN the Editor view is active and a document or list has focus
- WHEN the user presses `⌘S`, `⌘F`, `⇧⌘F`, or `⌘P`
- THEN save, find, content-search, or the quick-open palette triggers even while a field is focused

#### Scenario: Tab chords

- GIVEN the Editor view is active with two tabs open
- WHEN the user presses `⌘⌥→` or `⌘W`
- THEN the other tab activates, or the active clean tab closes and its neighbour activates; a dirty tab raises the guard

#### Scenario: Guarded plain keys

- GIVEN no checkout is selected, an editable field is focused, an IME composition is active, or a modal (draft guard, picker, context menu, quick-open) owns input
- WHEN the user presses `1`–`4`, `j`, `k`, or `Enter`
- THEN the shortcut does not fire; with a clean state `j`/`k` move only the highlight cursor and `Enter` activates through the row's existing handler

Evidence: `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/tabs.rs`, `apps/rune/src/editor/quick_open.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/main.rs`
````

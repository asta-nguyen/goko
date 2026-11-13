# Delta

Merged into docs/llm/: 2026-11-13

## ADDED Requirements

### RNE-changes-view-layout
Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; `RNE-changes-git-writes` covers the write actions and `RNE-history` the commit list, but no requirement covers a List/Tree view or grouping of the changed-file list.
Requirement:
````md
### RNE-changes-view-layout
The system SHALL let the Changes changed-file list and the History commit detail's changed-file list switch between a flat `List` and a directory `Tree` from their context menus — and, in Changes, from the header's view button — with one shared `List`/`Tree` choice that both sections read and both menus write (default `Tree`), and let the Changes list group its rows by `None`, `Tracked & Untracked`, or `Staged & Unstaged` (default `Staged & Unstaged`), keeping selection, patch loading, staging, commit, open-at-line, the filters, and keyboard navigation working in every view, with the choices kept for the app session and not persisted.
#### Scenario: Tree and List
- GIVEN the Changes list has a file under a directory
- WHEN the user chooses `Tree` in the context menu
- THEN a folder row holds the file and `List` returns the flat row
#### Scenario: The header view button
- GIVEN the Changes list is showing
- WHEN the user presses the view button beside the section's refresh
- THEN the same `View`/`Group By` menu opens beside the button and its choices apply
#### Scenario: Group By
- GIVEN a path is both staged and unstaged
- WHEN the user chooses `Group By: Staged & Unstaged`
- THEN a `Staged` group and an `Unstaged` group each show the path
#### Scenario: History detail
- GIVEN a commit detail is open in History
- WHEN the user chooses `Tree`
- THEN the commit's changed files nest under their directories
#### Scenario: One shared choice
- GIVEN the app is in the default `Tree`
- WHEN the user picks `List` in the Changes list menu and opens a commit in History
- THEN the History detail's changed files render flat, and picking `Tree` there turns the Changes list back to directory rows
#### Scenario: The choice lasts the session
- GIVEN the user picked `List` and `Group By: None`
- WHEN the user switches section and checkout and back
- THEN the choices hold until the app restarts
Evidence: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/commit_detail.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/navigation.rs`
````

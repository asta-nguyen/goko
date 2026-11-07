# Delta

Merged into docs/llm/: 2026-11-07

## ADDED Requirements

### EDT-history-table

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers the History tab, its graph, or its columns.

Requirement:

````md
### EDT-history-table

The system SHALL show the Editor's History as a one-line-per-commit table with a branch-coloured commit graph, a distinct HEAD node, and Graph, Description, Date, Author, and Commit columns whose widths and the table/detail divider the user can drag and the app restores on the next run.

#### Scenario: Resize a column and restore it

- GIVEN the History table is open
- WHEN the user drags the Author column edge wider and the app is reopened
- THEN the Author column opens at the dragged width

#### Scenario: Filter keeps the graph

- GIVEN several commits are loaded
- WHEN the user types a filter that matches only some of them
- THEN every row and the whole graph stay visible, the non-matching rows are dimmed, and Enter selects the next match

#### Scenario: Uncommitted changes row

- GIVEN the checkout has uncommitted changes and HEAD is loaded
- WHEN History opens with All branches selected
- THEN an "Uncommitted changes" row sits above HEAD and offers to open the Changes tab

Evidence: `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/HistoryTable.tsx`, `apps/desktop/src/features/editor/CommitDetail.tsx`, `apps/desktop/src/features/editor/history-graph.ts`, `apps/desktop/src/features/editor/history-layout.ts`, `apps/desktop/src/components/ui/resizable-handle.tsx`, `apps/desktop/src/features/editor/HistoryTable.test.tsx`, `apps/desktop/src/features/editor/history-graph.test.ts`, `apps/desktop/src/features/editor/history-layout.test.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````

### EDT-history-branch-filter

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers limiting History to a branch.

Requirement:

````md
### EDT-history-branch-filter

The system SHALL let the user limit History to one local or remote-tracking branch, or show all branches, accepting only a full branch ref that the checkout's branch list returned.

#### Scenario: Show one branch

- GIVEN the checkout has branches `main` and `feature`
- WHEN the user chooses `feature` in the branch dropdown
- THEN only commits reachable from `feature` are listed and the selection resets

#### Scenario: Unknown branch rejected

- GIVEN a request names a ref that is not under refs/heads or refs/remotes
- WHEN the backend receives it
- THEN it returns an invalid-input error and runs no `git log`

Evidence: `apps/desktop/src-tauri/src/git_view.rs`, `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````

### EDT-history-detail

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers the commit detail panel.

Requirement:

````md
### EDT-history-detail

The system SHALL show the selected commit's author with email, date, committer when it differs, parents, full message body, changed files with added and removed line counts, and its patch.

#### Scenario: Select a commit

- GIVEN a commit with a two-paragraph message that changed a text file and a binary file
- WHEN the user selects it
- THEN the panel shows both paragraphs, the text file with `+added −removed`, the binary file without counts, and the patch

#### Scenario: Jump to a parent

- GIVEN the selected commit's parent is loaded
- WHEN the user activates the parent's short hash
- THEN the parent becomes the selected commit

Evidence: `apps/desktop/src/features/editor/CommitDetail.tsx`, `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/panes.tsx`, `apps/desktop/src-tauri/src/git_view.rs`, `apps/desktop/src/features/editor/CommitDetail.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````

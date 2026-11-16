# Delta

Merged into docs/llm/: 2026-11-16

Approved: 2026-11-16 — open the working-tree file from changed-file rows (icon and context menu) and from each file header in the shared diff pane, across Changes, History, and Stashes

## ADDED Requirements

### RNE-open-changed-file

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers this.

Requirement:

````md
### RNE-open-changed-file

The system SHALL offer an "Open file" affordance on every changed-file row of the Changes list, the History commit detail's changed-file list, and the Stash detail's changed-file list — both as a row icon and as an `Open File` context-menu item opened by a secondary press on the row — and an open action on every file header in the shared diff pane, each opening the file's working-tree version through the same draft-guarded navigation as opening it from the file tree.

#### Scenario: Open from a row

- GIVEN a changed-file row in Changes, a commit detail, or a stash detail
- WHEN the user presses the row's open icon, or picks `Open File` from the row's context menu
- THEN the file opens on the Files code surface at its working-tree version, while a primary press on the row still selects its diff

#### Scenario: Open from the diff header

- GIVEN a diff is showing in Changes, History, or Stashes
- WHEN the user presses the open action in a file's diff header
- THEN that file opens on the Files code surface at its working-tree version

#### Scenario: File gone from the working tree

- GIVEN a row or diff header names a file deleted or renamed away in the working tree
- WHEN the user opens it
- THEN the open document shows the read-only missing-file state rather than an error

Evidence: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/commit_detail.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/patch.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/navigation.rs`
````

# Delta

Merged into docs/llm/: 2026-11-28

## MODIFIED Requirements

### RNE-sidebar-sync

Previously: entering the Editor view activated the checkout named by
`active_workspace`, which terminal session, tab and pane presses also move, so
returning from a terminal in another checkout switched the editor's checkout and
dropped its open tabs.

Baseline:

````md
### RNE-sidebar-sync

The system SHALL keep the chrome sidebar's committed workspace and the editor's checkout selection in sync — activating the committed checkout when the Editor view opens, decoding `active_workspace` into project and checkout, ignoring `__unassigned__`, unregistered or vanished project ids, and an already-matching selection, through the same draft-guarded `SelectCheckout` path, and clearing the committed workspace when a project switch drops the checkout so no stale row stays highlighted.

#### Scenario: Editor opens on the committed checkout

- GIVEN `active_workspace` names a registered project's checkout and the editor's selection is empty or different
- WHEN the user switches to the Editor view
- THEN `activate_sidebar_checkout` navigates `SelectCheckout` for that project+checkout and the sidebar highlight already matches it

#### Scenario: Unregistered workspace ignored

- GIVEN `active_workspace` is `__unassigned__`, names an unregistered or vanished project id, or no workspace is committed
- WHEN the Editor view opens
- THEN the view still switches but no project or checkout is selected

#### Scenario: Guarded view switch applies the sync after Discard

- GIVEN a dirty draft and `active_workspace` names another registered checkout
- WHEN the user switches to the Editor view and chooses Discard
- THEN the view becomes Editor with that project+checkout activated

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`
````

Replacement:

````md
### RNE-sidebar-sync

The system SHALL keep the chrome sidebar's project and worktree row presses and the editor's checkout selection in sync — every press commits that exact registered project root or listed worktree as the editor's project+checkout through the draft-guarded `SelectCheckout` path and records it as `active_workspace` only after the guard passes, so a cancelled or failed navigation restores the previous project, checkout, highlight, and draft — while entering the Editor view never derives the editor's checkout from `active_workspace`: a terminal session, tab or pane press may move the sidebar highlight, but the editor keeps its own checkout, open tabs and drafts, and a project switch that drops the checkout clears the committed workspace so no stale row stays highlighted.

#### Scenario: Sidebar row commits the editor checkout

- GIVEN the editor is committed to one project and checkout
- WHEN a project root or worktree row is pressed, in any view
- THEN `SelectCheckout` commits that exact registered project+checkout through the draft guard and the sidebar highlight matches it

#### Scenario: A terminal does not move the editor

- GIVEN the editor is on checkout A with a file open and `active_workspace` names checkout B after a terminal tab, session or pane press
- WHEN the user switches to the Editor view
- THEN the editor still shows checkout A with its open tab and find state, and the sidebar highlight stays on B

#### Scenario: No checkout derived on entry

- GIVEN the editor has no selected checkout and `active_workspace` names a registered checkout
- WHEN the Editor view opens
- THEN the view switches but no project or checkout is selected

#### Scenario: Guarded row press

- GIVEN the editor has a dirty draft
- WHEN a project or worktree row is pressed and the user chooses Cancel
- THEN project, checkout, highlight and draft are all unchanged; Discard or a landed Save commits the pending selection instead

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`
````

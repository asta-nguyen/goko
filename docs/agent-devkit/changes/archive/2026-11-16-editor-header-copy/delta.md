# Delta

Merged into docs/llm/: 2026-11-16

Approved: 2026-11-16 — the editor header's branch badge and checkout path chip become click-to-copy buttons (branch name, or the short hash when detached; the full checkout path), each writing the clipboard and showing the shared "Copied to clipboard" notice

## MODIFIED Requirements

### RNE-current-branch

Previously: the header badge only displayed the branch — it offered no action.

Baseline:

````md
### RNE-current-branch

The system SHALL show the current checkout's branch in the shared editor header across all sections, showing Detached with the short commit ID when detached, refreshing in the background while the editor is active, and showing loading or unavailable states instead of another checkout's branch or a failed cached value without changing Git state or document drafts.

#### Scenario: Branch stays visible

- GIVEN a checkout with a named branch is selected
- WHEN the user opens Files, Changes, History, Stashes, or Servers
- THEN the shared header displays that branch, truncating long names with the full name available on hover

#### Scenario: External switch or detached HEAD

- GIVEN the editor is active
- WHEN Git changes the checkout branch or detaches HEAD externally
- THEN a background worker refresh updates the header within the next three-second polling interval plus worker time, and detached HEAD includes its short commit ID

#### Scenario: Loading or failure

- GIVEN a checkout changes or its Git metadata cannot be read
- WHEN the header renders before a successful read
- THEN it shows loading or an unavailable state and does not present stale metadata as current

Evidence: `apps/rune/src/editor/branch.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`
````

Replacement:

````md
### RNE-current-branch

The system SHALL show the current checkout's branch in the shared editor header across all sections, showing Detached with the short commit ID when detached, refreshing in the background while the editor is active, showing loading or unavailable states instead of another checkout's branch or a failed cached value without changing Git state or document drafts, and letting a press on the badge copy the branch name — or the short commit ID when detached — to the clipboard with the shared "Copied to clipboard" notice.

#### Scenario: Branch stays visible

- GIVEN a checkout with a named branch is selected
- WHEN the user opens Files, Changes, History, Stashes, or Servers
- THEN the shared header displays that branch, truncating long names with the full name available on hover

#### Scenario: External switch or detached HEAD

- GIVEN the editor is active
- WHEN Git changes the checkout branch or detaches HEAD externally
- THEN a background worker refresh updates the header within the next three-second polling interval plus worker time, and detached HEAD includes its short commit ID

#### Scenario: Loading or failure

- GIVEN a checkout changes or its Git metadata cannot be read
- WHEN the header renders before a successful read
- THEN it shows loading or an unavailable state and does not present stale metadata as current

#### Scenario: Press to copy

- GIVEN the badge shows a named branch
- WHEN the user presses it
- THEN the full branch name lands on the clipboard and the "Copied to clipboard" notice shows; a detached badge copies its short commit ID, while loading and unavailable states stay inert

Evidence: `apps/rune/src/editor/branch.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`
````

## ADDED Requirements

### RNE-checkout-path-copy

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers this.

Requirement:

````md
### RNE-checkout-path-copy

The system SHALL let a press on the checkout path chip in the shared editor header copy the full checkout path to the clipboard and show the shared "Copied to clipboard" notice.

#### Scenario: Press the path chip

- GIVEN a checkout is selected and its path chip is visible in the editor header
- WHEN the user presses it
- THEN the full checkout path lands on the clipboard and the "Copied to clipboard" notice shows

Evidence: `apps/rune/src/editor/workspace.rs`
````

# Delta

Merged into docs/llm/: 2026-11-21

Approved: 2026-11-21 — add a cross-workspace Needs you picker that opens the selected waiting terminal through the draft guard

## ADDED Requirements

### RNC-needs-you

Page: `docs/llm/domains/rune-chrome.md`
Search: checked Rune chrome and the registered RNC requirements; workspace attention shows counts but offers no direct session picker.

Requirement:

````md
### RNC-needs-you

The system SHALL offer a Needs you picker beside the workspace selector when any live session has the existing Waiting activity, listing waiting sessions across workspaces with their location and opening the chosen session's terminal pane through the editor draft guard.

#### Scenario: choose a session in another workspace

- GIVEN a waiting agent in a split tab in another workspace
- WHEN the user picks it from Needs you
- THEN the app selects its workspace and opens that tab with the agent's pane focused

#### Scenario: cancel navigation

- GIVEN an unsaved editor draft
- WHEN the user picks a waiting session and cancels the draft guard
- THEN the current editor, workspace and terminal selection stay unchanged

#### Scenario: waiting ends

- GIVEN a session is in the waiting list
- WHEN the session stops waiting or exits
- THEN it leaves the list, and a pick still pending behind the draft guard does not open it

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/view_tests.rs`
````

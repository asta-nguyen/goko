# Delta

Approved: 2026-11-12 — Show the focused terminal pane’s Git branch beside the header path; refresh on cwd/focus changes and every three seconds, hide outside repositories and on failures, show short detached HEAD.

## ADDED Requirements

### RNC-terminal-current-branch

Page: `docs/llm/domains/rune-chrome.md`
Search: checked Rune chrome requirements; none covers a live terminal current-branch badge.

Requirement:

````md
### RNC-terminal-current-branch

The system SHALL display the focused Terminal pane’s current Git branch beside the header path, read off the UI thread and refreshed on pane/cwd changes and every three seconds while Terminal is active, hiding stale or failed results and paths outside Git repositories.

#### Scenario: focused pane changes

- GIVEN terminal panes have different working directories
- WHEN focus or cwd changes
- THEN the badge uses only the focused pane’s cwd and never displays the previous target’s branch

#### Scenario: Git identity changes

- GIVEN Terminal is active in a Git repository
- WHEN Git switches branch or detaches HEAD
- THEN the next refresh displays the branch name or Detached with the short commit ID

#### Scenario: unavailable identity

- GIVEN the focused cwd is outside a Git repository or its Git read fails
- WHEN the read completes
- THEN no branch badge is displayed

Evidence: `apps/rune/src/terminal_branch.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`
````

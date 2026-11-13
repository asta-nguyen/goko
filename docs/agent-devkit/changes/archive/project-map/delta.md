# Project Map delta

Merged into docs/llm/: 2026-11-13

## ADDED Requirements

### RNC-project-map

Page: `docs/llm/domains/rune-chrome.md`
Search: searched the sole page registered under prefix RNC, its existing workspace/shortcut/selection requirements, the retired-ID registry, and open change artifacts; no existing requirement defines Project Map or claims this ID.

Requirement:

````md
### RNC-project-map

The system SHALL provide a searchable, keyboard-accessible Workspace → Project map from plain `⌘K` or a sidebar button in Terminal/Editor, with workspace-only selection preserving the checkout and project selection atomically choosing its current owning workspace and root checkout through the draft guard while preserving the app view, terminal tabs and sessions.

#### Scenario: inspect and search membership

- GIVEN registered projects in Default and named workspaces, including duplicate project names
- WHEN Project Map opens and a trimmed case-insensitive workspace name, project name or root path is entered
- THEN matching projects appear under their owning workspace headings with distinguishing paths, a workspace-name match includes that workspace's projects, and current workspace/project markers remain distinct from the keyboard cursor

#### Scenario: select a workspace

- GIVEN the map shows a valid workspace heading
- WHEN the user clicks it or activates it with Enter
- THEN the map closes, the sidebar selects and remembers that workspace through the existing settings path, and the checkout, view, tabs and sessions stay unchanged

#### Scenario: jump to a project

- GIVEN a project belongs to another workspace and no dirty draft blocks selection
- WHEN its map row is clicked or activated with Enter
- THEN the map closes, its current owning workspace is selected, its root checkout becomes active, the sidebar reveals its row, the existing view remains unchanged, and terminal visibility and selection follow the active project as described in [RNC-project-terminals](#rnc-project-terminals)

#### Scenario: cancel or fail a draft save

- GIVEN a dirty draft and a project jump requiring the draft guard
- WHEN Cancel is chosen or Save fails
- THEN the prior workspace, checkout and document remain selected and no destination workspace setting is written

#### Scenario: membership changes while a guard waits

- GIVEN a project was chosen and the draft guard is waiting
- WHEN the project moves to another workspace before navigation resumes
- THEN navigation uses its current owning workspace; a removed project or missing owner instead shows a notice and changes neither selection

#### Scenario: map owns input and dismisses

- GIVEN the map is open
- WHEN the user types, pastes, uses Up/Down or Enter, or dismisses with Escape, close or scrim
- THEN input stays in the map, navigation stays visible, dismissal restores usable prior focus without switching selection, and underlying terminal/editor actions do not run

#### Scenario: unavailable data and shortcut ownership

- GIVEN initial lists are loading, reads failed, results are empty, or another modal/terminal composition owns input
- WHEN the map is displayed or plain `⌘K` is attempted
- THEN loading/error/empty/no-results states remain distinguishable, unready initial data cannot be selected, failed reads can retry through existing refresh requests, and the shortcut does not override another input owner or open from Settings

Evidence: `apps/rune/src/project_map.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`
````

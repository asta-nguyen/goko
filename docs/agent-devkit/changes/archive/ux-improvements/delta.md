# Delta

Merged into docs/llm/: 2026-11-11

## ADDED Requirements

### RNC-sidebar-accessible-names

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; only `RNC-header-status` names an accessible control (the wordmark group); no requirement covers sidebar control names.

Requirement:

````md
### RNC-sidebar-accessible-names

The system SHALL give every sidebar button an accessible name that includes its target: disclosure buttons read "Collapse <name>" or "Expand <name>", and row actions read "New session in <row name>", "More actions for <row name>", "Stop session <title>", and "Close pane <title>", with " (2)", " (3)" appended to repeated names so none is shared.

#### Scenario: no unnamed or duplicate names

- GIVEN a sidebar with several projects, a worktree group, and a session
- WHEN the accessibility tree is read
- THEN no sidebar button has an empty name and no two action buttons share a name

Evidence: `apps/rune/src/sidebar.rs`
````

### RNC-sidebar-status-help

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNC-usage-accounts` defines the chips but no requirement covers explanatory help text on status marks.

Requirement:

````md
### RNC-sidebar-status-help

The system SHALL explain each sidebar status mark on hover: a warning dot reads "Uncommitted changes", a muted dot "Git status unknown", the `missing` badge "This folder no longer exists on disk", a truncated label shows its full text, and a usage chip states that its percentage is the share of that window already used.

#### Scenario: hover a dirty checkout

- GIVEN a checkout whose `dirty` value is true
- WHEN the pointer rests on its dot
- THEN the help text reads "Uncommitted changes"

#### Scenario: truncated worktree name

- GIVEN a worktree label wider than its cell
- WHEN the pointer rests on it
- THEN the help text is the complete label

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/theme.rs`, `apps/rune/src/chrome.rs`
````

### RNC-notice-expiry

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; no requirement describes the global notice toast's lifetime.

Requirement:

````md
### RNC-notice-expiry

The system SHALL close the global notice toast 8 seconds after it was last set, restart that timer when a notice with different text replaces it, and keep the toast's close button.

#### Scenario: expiry and replacement

- GIVEN a notice shown at time 0
- WHEN a second notice with different text replaces it at 7 seconds
- THEN the first does not close at 8 seconds and the second closes at 15 seconds

Evidence: `apps/rune/src/main.rs`
````

### RNC-remove-confirm

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNC-sidebar-row-actions` says Remove worktree is "chosen and confirmed" but no requirement defines the confirmation, and none covers Remove project.

Requirement:

````md
### RNC-remove-confirm

The system SHALL ask for confirmation in an inline sidebar row before Remove worktree and Remove project run: the worktree row names the folder, says whether it has uncommitted changes (refreshed when the row opens), and states how many sessions that have not ended run in it, while the project row states that only the registration is removed, its sessions become Unassigned, and its files stay; Cancel changes nothing, and running sessions are never stopped.

#### Scenario: confirm a dirty worktree removal

- GIVEN a worktree row whose `dirty` value is true with two running sessions attached
- WHEN Remove worktree is chosen
- THEN an inline row names the folder, shows the uncommitted-changes line and "2 sessions", and nothing is removed until Confirm is pressed

#### Scenario: cancel

- GIVEN a removal confirmation row is open
- WHEN Cancel is pressed
- THEN no request is enqueued and the row closes

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/main.rs`
````

### RNC-empty-terminal-actions

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNC-tab-close` and `RNC-terminal-attach` do not describe the view with no open tab.

Requirement:

````md
### RNC-empty-terminal-actions

The system SHALL show, in the Terminal view with no open tab, one start button per available agent plus Shell for the active editor checkout (else the first project visible in the selected workspace), or an Add Project button when the selected workspace has no project, with the buttons disabled and a reason shown while the daemon is disconnected.

#### Scenario: first launch with projects

- GIVEN no open tab, two available agents, and a registered project
- WHEN the Terminal view renders
- THEN it shows a button for each agent and Shell

#### Scenario: no projects

- GIVEN no open tab and an empty selected workspace
- WHEN the Terminal view renders
- THEN it shows only Add Project

Evidence: `apps/rune/src/panes.rs`, `apps/rune/src/sessions.rs`
````

### RNC-new-tab-menu

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNC-new-tab` defines New tab as a shell and stays unchanged; no requirement covers an agent menu or unavailable-agent reasons.

Requirement:

````md
### RNC-new-tab-menu

The system SHALL offer, beside New tab, a menu listing every agent preset and Custom command… that starts that session in the active checkout and shows each unavailable agent as disabled with the reason "<binary> not found on PATH" in this menu and in the sidebar's New session menu.

#### Scenario: start an agent from the tab bar

- GIVEN an available agent and an active checkout
- WHEN the agent is chosen from the New tab menu
- THEN `session.create` runs for that agent in the active checkout

#### Scenario: agent not installed

- GIVEN an agent whose binary is not on PATH
- WHEN either menu opens
- THEN its entry is disabled and states "<binary> not found on PATH"

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `crates/app-core/src/agents.rs`
````

### RNC-app-shortcuts

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNE-keyboard` covers only editor chords; no chrome requirement defines app shortcuts.

Requirement:

````md
### RNC-app-shortcuts

The system SHALL bind `⌘T` to a new shell tab, `⌘1`–`⌘9` to the Nth tab in the Terminal view, and `⌘,` to the Settings toggle through the draft guard, ignoring them while the sidebar prompt, a removal confirmation, a sidebar or editor menu or picker, the draft guard, quick-open, or a terminal input-method composition owns input and for any chord that adds Shift or Option, leaving `⌘W` as window close.

#### Scenario: switch tab by number

- GIVEN two open tabs in the Terminal view
- WHEN `⌘2` is pressed
- THEN the second tab is active, and `⌘9` does nothing

#### Scenario: shortcut blocked by a confirmation

- GIVEN a removal confirmation is open
- WHEN `⌘T` is pressed
- THEN no session starts

Evidence: `apps/rune/src/main.rs`, `apps/rune/src/panes.rs`, `apps/rune/src/chrome.rs`
````

### RNC-prune-worktrees

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; `RNC-sidebar-row-actions` lists the More menu entries but none for pruning.

Requirement:

````md
### RNC-prune-worktrees

The system SHALL offer Prune missing worktrees in a git project's More menu only while at least one of its worktrees is `missing`, run `git worktree prune` in the project root when chosen, refresh that project's worktree list, and touch no folder or session.

#### Scenario: prune

- GIVEN a project with a worktree whose folder was deleted
- WHEN Prune missing worktrees is chosen
- THEN Git's record of that worktree is removed, the list refreshes, and a live linked worktree remains

#### Scenario: nothing to prune

- GIVEN a project with no missing worktree
- WHEN its More menu opens
- THEN the entry is absent

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/worktrees.rs`
````

### RNC-workspace-attention

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC (including `RNC-project-workspaces`); no requirement surfaces sessions hidden by the workspace filter.

Requirement:

````md
### RNC-workspace-attention

The system SHALL show a warning dot on the sidebar's workspace selector while any session in a workspace other than the selected one is waiting for the user, and append "· N need you" to that workspace's entry in the selector menu, using the same activity value as the session row's dot.

#### Scenario: a hidden session needs the user

- GIVEN two workspaces and a session waiting for the user in the one that is not selected
- WHEN the sidebar renders
- THEN the selector shows the warning dot and the other workspace's menu entry reads "· 1 need you"

#### Scenario: the session stops waiting

- GIVEN that session finishes its turn
- WHEN the sidebar renders
- THEN the dot and the suffix are gone

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/sessions.rs`
````

### RNE-empty-state-projects

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; `RNE-checkout-selection` covers selection but not the empty Editor.

Requirement:

````md
### RNE-empty-state-projects

The system SHALL list up to five registered projects (name and root) in the Editor when no checkout is selected, selecting a project's root checkout through the draft-guarded navigation when one is pressed, alongside the Add project button.

#### Scenario: pick a project

- GIVEN no checkout is selected and three registered projects
- WHEN the Editor renders and the second row is pressed
- THEN that project's root checkout is selected through the draft guard

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`
````

### RNE-notice-dismiss

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; error paths mention notices but no requirement says the notice bar can be dismissed.

Requirement:

````md
### RNE-notice-dismiss

The system SHALL show a dismiss button on the Editor's notice bar that clears the notice at once.

#### Scenario: dismiss a stale notice

- GIVEN the notice bar shows a stale-data notice
- WHEN its dismiss button is pressed
- THEN the bar disappears

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/state.rs`
````

## MODIFIED Requirements

### RNC-header-status

Previously: the pill read "Disconnected" and carried no explanation.

Baseline:

````md
### RNC-header-status

The system SHALL render the header with the goko mark and wordmark, a persistent
6 px connection dot in a 12 px halo (success while the daemon client is
connected, danger otherwise) whose wordmark group carries an accessible name
and hover tooltip ("Connected to the Goko daemon" / "Disconnected from the
Goko daemon"), one usage chip per `Data::usage` entry, and a settings button;
a "Disconnected" pill appears on the right only while disconnected, entering
with a short fade and small scale-up.

#### Scenario: daemon connected

- GIVEN the daemon client completed the hello handshake
- WHEN the view renders
- THEN the connection dot is success-coloured and no Disconnected pill is shown

#### Scenario: daemon unreachable

- GIVEN the daemon client failed to connect or lost the socket
- WHEN the view renders
- THEN the connection dot is danger-coloured and the Disconnected pill renders beside the settings button

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/data.rs`
````

Replacement:

````md
### RNC-header-status

The system SHALL render the header with the goko mark and wordmark, a persistent
6 px connection dot in a 12 px halo (success while the daemon client is
connected, danger otherwise) whose wordmark group carries an accessible name
and hover tooltip ("Connected to the Goko daemon" / "Disconnected from the
Goko daemon"), one usage chip per `Data::usage` entry, and a settings button;
a "Reconnecting…" pill appears on the right only while disconnected, entering
with a short fade and small scale-up, whose help text is the daemon's last
disconnect reason or "Waiting for the Goko daemon" when there is none, with no
manual retry control.

#### Scenario: daemon connected

- GIVEN the daemon client completed the hello handshake
- WHEN the view renders
- THEN the connection dot is success-coloured and no Reconnecting… pill is shown

#### Scenario: daemon unreachable

- GIVEN the daemon client failed to connect or lost the socket
- WHEN the view renders
- THEN the connection dot is danger-coloured and the Reconnecting… pill renders beside the settings button with the last reason as its help text

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/data.rs`
````

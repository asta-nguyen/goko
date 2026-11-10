# Delta

Approved: 2026-11-10 — the user asked for the plan to be implemented directly.

## MODIFIED Requirements

### RNC-controls-bar

Previously: the strip's Stop button is dimmed when the state is not controllable or the mock is disconnected, and session data is mock.

Baseline:

````md
### RNC-controls-bar

The system SHALL render the controls strip only in the Terminal view while a
session is selected — the Editor view has no session pane, matching App.tsx —
showing the project/agent/title breadcrumb, a state badge with an
activity-aware tone dot, the elapsed time, and Stop/Close pane buttons; Stop is
dimmed when the state is not controllable or the mock is disconnected.

#### Scenario: session selected in Terminal view

- GIVEN `view` is `Terminal` and `selected_session` names a `SessionView`
- WHEN the view renders
- THEN the strip shows the breadcrumb, badge, elapsed, and both buttons

#### Scenario: no strip in Editor or without a session

- GIVEN `view` is `Editor`, or `selected_session` is `None`
- WHEN the view renders
- THEN the controls strip renders nothing

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`
````

Replacement:

````md
### RNC-controls-bar

The system SHALL render the controls strip only in the Terminal view while a
daemon session is selected — the Editor view has no session pane, matching
App.tsx — showing the project/agent/title breadcrumb resolved from the real
`SessionInfo` and its stored `SessionMeta`, a state badge with a tone dot
(spinning accent ring while the session works), the elapsed time, and
Stop/Close pane buttons that issue `session.stop`/`session.delete` to the
daemon; Stop is dimmed when the state is not controllable or the daemon is
disconnected.

#### Scenario: session selected in Terminal view

- GIVEN `view` is `Terminal` and `selected_session` names a live `SessionView`
- WHEN the view renders
- THEN the strip shows the breadcrumb, badge, elapsed, and both buttons

#### Scenario: no strip in Editor or without a session

- GIVEN `view` is `Editor`, or `selected_session` is `None`
- WHEN the view renders
- THEN the controls strip renders nothing

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/main.rs`
````

### RNC-header-status

Previously: the connection dot reads the mock `Data::connected` flag, always true.

Baseline:

````md
### RNC-header-status

The system SHALL render the header with the goko mark and wordmark, a persistent
6 px connection dot in a 12 px halo (success when `Data::connected`, danger
otherwise) whose wordmark group carries an accessible name and hover tooltip
("Connected to the Goko daemon" / "Disconnected from the Goko daemon"), one
usage chip per `Data::usage` entry, and a settings button; a "Disconnected" pill
appears on the right only while `connected` is false, entering with a short fade
and small scale-up.

#### Scenario: connected mock

- GIVEN `Data::connected` is `true`
- WHEN the view renders
- THEN the connection dot is success-coloured and no Disconnected pill is shown

#### Scenario: disconnected mock

- GIVEN `Data::connected` is `false`
- WHEN the view renders
- THEN the connection dot is danger-coloured and the Disconnected pill renders beside the settings button

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`
````

Replacement:

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

### RNC-new-tab

Previously: New tab spawns a mock `shell` session titled `Shell N`.

Baseline:

````md
### RNC-new-tab

The system SHALL create a workspace tab when New tab is pressed: it spawns a
mock running `shell` session via `data::spawn_shell` titled `Shell N`, pushes
the title onto `open_tabs`, makes it active, and selects the session.

#### Scenario: press New tab

- GIVEN `open_tabs` holds several titles
- WHEN New tab is pressed
- THEN a `Shell N` session is appended to `Data::sessions`, a matching tab opens and activates, and `selected_session`/`active_workspace` follow it

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`
````

Replacement:

````md
### RNC-new-tab

The system SHALL create a workspace tab when New tab is pressed: it issues
`session.create` to the daemon for a shell in the active checkout, records the
session metadata through the app store, opens a tab titled from the session,
makes it active, and selects the session once `session.updated` lands; a
failed spawn surfaces an error and opens no phantom tab.

#### Scenario: press New tab

- GIVEN `open_tabs` holds several titles and the daemon is connected
- WHEN New tab is pressed
- THEN `session.create` runs for the active checkout, the session metadata is recorded, a matching tab opens and activates, and `selected_session` follows the real `SessionInfo`

#### Scenario: spawn fails

- GIVEN the daemon rejects `session.create`
- WHEN New tab is pressed
- THEN an error notice shows and no tab or session row appears

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/data.rs`, `crates/app-core/src/store.rs`, `crates/app-core/src/agents.rs`
````

### RNC-session-actions

Previously: Stop/Close pane mutate mock session state.

Baseline:

````md
### RNC-session-actions

The system SHALL make the controls strip's buttons mutate mock session state:
Stop marks a live selected session `Exited` with `ended_at` frozen at
`MOCK_NOW_MS` and clears its activity, and Close pane removes the session from
`Data::sessions` plus its title from `open_tabs`, then re-syncs selection to
the active tab.

#### Scenario: stop a running session

- GIVEN the selected session's state is `Running`
- WHEN Stop is pressed
- THEN its state becomes `Exited`, `ended_at` is set, and the badge/elapsed freeze

#### Scenario: close the selected pane

- GIVEN the selected session's title is open as a tab
- WHEN Close pane is pressed
- THEN the session and tab are removed and selection moves to the active tab's session

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`
````

Replacement:

````md
### RNC-session-actions

The system SHALL make the controls strip's buttons drive the daemon: Stop
issues `session.stop` for the selected session and Close pane issues
`session.delete`, removes the session's stored metadata, drops its title from
`open_tabs`, and re-syncs selection to the active tab; both reflect the real
`session.updated`/`session.exited` events rather than mutating local state.

#### Scenario: stop a running session

- GIVEN the selected session's state is `Running`
- WHEN Stop is pressed
- THEN `session.stop` is sent and the badge/elapsed update when the daemon reports the new state

#### Scenario: close the selected pane

- GIVEN the selected session's title is open as a tab
- WHEN Close pane is pressed
- THEN `session.delete` is sent, the metadata row and tab are removed, and selection moves to the active tab's session

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/data.rs`, `crates/app-core/src/store.rs`
````

### RNC-sidebar-projects

Previously: session rows render mock `Data::sessions` nested by cwd matching.

Baseline:

````md
### RNC-sidebar-projects

The system SHALL render the project sidebar from the editor backend's
registered projects and their per-project worktree lists — never the mock
`Data::projects` — with one expandable row per project (chevron, folder icon,
name, missing badge, checkout status, branch label), a collapsible "Worktrees"
group of linked checkouts on git repositories, session rows nested under the
checkout their cwd resolves to, an "Other sessions" group for git-project
sessions matching no checkout, an "Unassigned" group, honest loading, error,
or empty status rows instead of fabricated projects, and an "Add Project"
button that registers a natively picked folder through the editor backend's
`Req::AddProject`.

#### Scenario: real projects, worktrees, and unassigned sessions

- GIVEN the Projects reply lists registered projects and per-project worktree
  lists have landed
- WHEN the sidebar renders
- THEN each project shows its root row and, for git repos, a Worktrees group
  of linked checkouts; a failed worktree read renders under its own project
  row only; and every mock session renders under Unassigned because mock
  project ids are never registered

#### Scenario: honest status rows

- GIVEN no projects are loaded yet, the Projects read failed, or the registry
  is empty
- WHEN the sidebar renders
  THEN it shows "Loading projects…", the error text, or the empty-registry
  hint — never a fabricated project row

#### Scenario: add a project

- GIVEN the sidebar header
- WHEN "Add Project" is pressed and a folder is chosen in the native dialog
- THEN `Req::AddProject` registers it through the editor backend and the
  refreshed project list renders the new row

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/worktrees.rs`
````

Replacement:

````md
### RNC-sidebar-projects

The system SHALL render the project sidebar from the editor backend's
registered projects and their per-project worktree lists — never the mock
`Data::projects` — with one expandable row per project (chevron, folder icon,
name, missing badge, checkout status, branch label), a collapsible "Worktrees"
group of linked checkouts on git repositories, real daemon session rows nested
under the checkout their cwd resolves to, an "Other sessions" group for
git-project sessions matching no checkout, an "Unassigned" group, honest
loading, error, or empty status rows instead of fabricated projects, and an
"Add Project" button that registers a natively picked folder through the
editor backend's `Req::AddProject`.

#### Scenario: real projects, worktrees, and daemon sessions

- GIVEN the Projects reply lists registered projects, per-project worktree
  lists have landed, and `session.list` returned live sessions
- WHEN the sidebar renders
- THEN each project shows its root row and, for git repos, a Worktrees group
  of linked checkouts; a failed worktree read renders under its own project
  row only; and each session nests under the checkout its cwd resolves to,
  falling back to Other sessions or Unassigned

#### Scenario: honest status rows

- GIVEN no projects are loaded yet, the Projects read failed, or the registry
  is empty
- WHEN the sidebar renders
  THEN it shows "Loading projects…", the error text, or the empty-registry
  hint — never a fabricated project row

#### Scenario: add a project

- GIVEN the sidebar header
- WHEN "Add Project" is pressed and a folder is chosen in the native dialog
- THEN `Req::AddProject` registers it through the editor backend and the
  refreshed project list renders the new row

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/worktrees.rs`
````

### RNC-tab-activate

Previously: tab presses select sessions whose titles match mock data.

Baseline:

````md
### RNC-tab-activate

The system SHALL activate a workspace tab when its pill is pressed, setting
`active_tab` to that index and selecting the session whose title matches, which
also sets `active_workspace` to that session's resolved workspace —
`data::workspace_for_session` returns `__unassigned__` for every current mock
session because their project ids are never registered projects, so session
selection highlights Unassigned and never fabricates an editor checkout. A
hovered inactive tab washes lighter and lifts its title, and stays visibly
distinct from the active tab; tabs are keyed by title so neighbours slide into
place when one closes.

#### Scenario: press a tab

- GIVEN `open_tabs` holds several titles and `active_tab` points elsewhere
- WHEN the user presses a tab pill
- THEN `active_tab` becomes that index and `selected_session`/`active_workspace`
  follow the matching `SessionView` through `workspace_for_session`

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`
````

Replacement:

````md
### RNC-tab-activate

The system SHALL activate a workspace tab when its pill is pressed, setting
`active_tab` to that index and selecting the daemon session it names, which
also sets `active_workspace` to that session's resolved workspace through
`data::workspace_for_session` — sessions whose cwd matches a registered
checkout highlight it, others resolve to `__unassigned__`. A hovered inactive
tab washes lighter and lifts its title, and stays visibly distinct from the
active tab; tabs are keyed by title so neighbours slide into place when one
closes.

#### Scenario: press a tab

- GIVEN `open_tabs` holds several titles and `active_tab` points elsewhere
- WHEN the user presses a tab pill
- THEN `active_tab` becomes that index and `selected_session`/`active_workspace`
  follow the matching `SessionView` through `workspace_for_session`

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`
````

### RNC-tab-close

Previously: closing a tab removes it and reselects the mock session of the newly active tab.

Baseline:

````md
### RNC-tab-close

The system SHALL remove a workspace tab when its close control is pressed,
re-indexing `active_tab` and reselecting the session of the tab that becomes
active — clearing `selected_session`/`active_workspace` when no session
resolves.

#### Scenario: close the active tab

- GIVEN three open tabs with the last one active
- WHEN its close control is pressed
- THEN the tab is removed, `active_tab` clamps to the new last index, and the matching session is selected

Evidence: `apps/rune/src/chrome.rs`
````

Replacement:

````md
### RNC-tab-close

The system SHALL remove a workspace tab when its close control is pressed,
re-indexing `active_tab` and reselecting the session of the tab that becomes
active — clearing `selected_session`/`active_workspace` when no session
resolves; closing a tab whose session is still running asks the daemon to
stop it only through the explicit Close pane action, never implicitly.

#### Scenario: close the active tab

- GIVEN three open tabs with the last one active
- WHEN its close control is pressed
- THEN the tab is removed, `active_tab` clamps to the new last index, and the matching session is selected

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/daemon.rs`
````

### RNC-view-switch

Previously: switching back to Terminal renders the ANSI transcript stub.

Baseline:

````md
### RNC-view-switch

The system SHALL switch between Terminal and the native Editor workspace through the view-switch radio group, showing the real-checkout Files, Changes, History, Stashes, and Servers views in Editor while guarding navigation away from an unsaved draft with Save, Discard or Cancel and preserving the switch's keyboard and motion behavior.

#### Scenario: switch to editor

- GIVEN `view` is `Terminal`
- WHEN the Editor item is pressed
- THEN `view` becomes `Editor` and the native editor offers real registered-project and checkout selection

#### Scenario: cancel navigation away from a draft

- GIVEN `view` is `Editor` and the current file has unsaved changes
- WHEN the Terminal item is pressed and the user chooses Cancel
- THEN the editor and its draft remain active; a failed Save also keeps the editor and draft active

#### Scenario: switch back after resolving a draft

- GIVEN `view` is `Editor` and there is no dirty draft, or Save/Discard has resolved it
- WHEN navigation to Terminal proceeds
- THEN `view` becomes `Terminal` and the existing terminal stub renders again

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`
````

Replacement:

````md
### RNC-view-switch

The system SHALL switch between Terminal and the native Editor workspace through the view-switch radio group, showing the real-checkout Files, Changes, History, Stashes, and Servers views in Editor while guarding navigation away from an unsaved draft with Save, Discard or Cancel and preserving the switch's keyboard and motion behavior; the Terminal view renders the attached session's live PTY grid.

#### Scenario: switch to editor

- GIVEN `view` is `Terminal`
- WHEN the Editor item is pressed
- THEN `view` becomes `Editor` and the native editor offers real registered-project and checkout selection

#### Scenario: cancel navigation away from a draft

- GIVEN `view` is `Editor` and the current file has unsaved changes
- WHEN the Terminal item is pressed and the user chooses Cancel
- THEN the editor and its draft remain active; a failed Save also keeps the editor and draft active

#### Scenario: switch back after resolving a draft

- GIVEN `view` is `Editor` and there is no dirty draft, or Save/Discard has resolved it
- WHEN navigation to Terminal proceeds
- THEN `view` becomes `Terminal` and the attached session's terminal renders

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`
````

## ADDED Requirements

### RNC-sidebar-row-actions

The system SHALL reveal per-row hover actions matching the desktop: project
and worktree rows offer a New-session menu (agent presets discovered through
`app_core::agents`, plus a custom command) that issues `session.create` in
that checkout and records session metadata, and a "more actions" menu
(Open Editor, Refresh worktrees, Create worktree, Remove worktree on
worktree rows; Open Editor, Create worktree, Refresh worktrees, Remove
project on project rows) routed through the editor backend's guarded lanes;
session rows offer Stop and Close pane mapped to `session.stop` /
`session.delete`.

#### Scenario: spawn a session from a project row

- GIVEN a registered project row with its hover actions revealed
- WHEN New session → an agent is chosen
- THEN `session.create` runs with the agent's resolved argv in the project
  checkout, metadata is recorded, and the session appears nested under it

#### Scenario: remove a worktree

- GIVEN a worktree row's more-actions menu
- WHEN Remove worktree is chosen and confirmed
- THEN the worktree is removed through git, the list refreshes, and the
  editor's stale-checkout handling applies if it was open

#### Scenario: actions hidden until hover

- GIVEN the sidebar renders
- WHEN no row is hovered
- THEN no action menu is visible; hovering a row reveals its actions without
  shifting layout

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/agents.rs`, `crates/app-core/src/worktrees.rs`, `crates/app-core/src/projects.rs`

### RNC-sidebar-resize

The system SHALL let the user drag the sidebar's right-edge separator to
resize it between 220 and 480 px, persisting the chosen width through the
app settings store so it survives restarts.

#### Scenario: drag within bounds

- GIVEN the sidebar at its default width
- WHEN the separator is dragged
- THEN the width follows the pointer clamped to 220–480 px and the setting is
  saved

#### Scenario: width restored on launch

- GIVEN a previously persisted width of 300 px
- WHEN the app starts
- THEN the sidebar opens at 300 px

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/store.rs`

### RNC-terminal-attach

The system SHALL attach the Terminal view to the selected daemon session:
`terminal.subscribe` replays retained output then streams live kind-1 frames
into the alacritty `Term` grid rendered as styled runs; keyboard input
encodes to `terminal.write`, viewport changes send `terminal.resize`,
`terminal.lagged` or a session switch resubscribes, `terminal.unsubscribe`
runs on detach, and an exited session keeps its final grid with input
suppressed.

#### Scenario: attach and interact

- GIVEN a running session selected in the Terminal view
- WHEN output frames arrive
- THEN the grid renders them; typing sends bytes the session receives (echo
  returns through the same path)

#### Scenario: lagged subscriber recovers

- GIVEN the terminal subscriber fell behind
- WHEN the daemon sends `terminal.lagged`
- THEN the client unsubscribes and resubscribes, replaying from the retained
  buffer

#### Scenario: switch sessions

- GIVEN session A attached
- WHEN session B is selected
- THEN A is unsubscribed, B subscribes, and B's retained+live output renders

Evidence: `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/main.rs`, `crates/protocol/src/frame.rs`, `crates/protocol/src/message.rs`

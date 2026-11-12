# Rune chrome

The `goko-rune` app (`apps/rune`, Rune/wgpu/winit) renders the Goko shell chrome —
the 48 px header, the project sidebar, the workspace tab strip, and the session
controls strip. Sessions and tabs are real daemon sessions joined with
their stored metadata: `session.create`/`stop`/`delete` requests drive the
row actions, split panes and close lifecycle
`Data`/`State` (spawn shell sessions, stop or close them, switch workspace
views). The project sidebar is the real exception: it lists registered projects
and their Git worktrees from the editor backend, and its selection commits to
the Editor workspace through the draft guard — see
[RNC-sidebar-projects](#rnc-sidebar-projects) and
[RNC-sidebar-selection](#rnc-sidebar-selection). The terminal stub is a
separate region outside this page's scope. The Editor view mounts the native
editor workspace documented in [Rune editor](rune-editor.md), which performs
real project, Git, file, and ACP work through `app-core`.

## Requirements

### RNC-app-shortcuts

The system SHALL bind `⌘T` to a new shell tab, `⌘1`–`⌘9` to the Nth visible tab in the Terminal view, and `⌘,` to the Settings toggle through the draft guard, ignoring them while the sidebar prompt, a removal confirmation, a sidebar or editor menu or picker, the draft guard, quick-open, or a terminal input-method composition owns input and for any chord that adds Shift or Option, leaving `⌘W` as window close.

#### Scenario: switch tab by number

- GIVEN two open tabs in the Terminal view
- WHEN `⌘2` is pressed
- THEN the second tab is active, and `⌘9` does nothing

#### Scenario: shortcut blocked by a confirmation

- GIVEN a removal confirmation is open
- WHEN `⌘T` is pressed
- THEN no session starts

Evidence: `apps/rune/src/main.rs`, `apps/rune/src/panes.rs`, `apps/rune/src/chrome.rs`

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

### RNC-notice-expiry

The system SHALL close the global notice toast 8 seconds after it was last set, restart that timer when a notice with different text replaces it, and keep the toast's close button.

#### Scenario: expiry and replacement

- GIVEN a notice shown at time 0
- WHEN a second notice with different text replaces it at 7 seconds
- THEN the first does not close at 8 seconds and the second closes at 15 seconds

Evidence: `apps/rune/src/main.rs`

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

### RNC-project-terminals

The system SHALL show only the active project's session rows and terminal tabs, including its worktrees, remember its active tab in memory, preserve other projects' running sessions and tab layouts, and apply numbered tab shortcuts to the visible tabs; without an active project the existing unfiltered view remains available.

#### Scenario: switch projects and return

- GIVEN projects A and B have open tabs and A is active
- WHEN B is selected through the sidebar, editor picker or Project Map, then A is selected again
- THEN each selection shows only that project's sessions and tabs and restores its remembered tab and pane without stopping or deleting other sessions

#### Scenario: project with no tab

- GIVEN A has a selected terminal and B has no open tab
- WHEN B becomes active
- THEN the terminal start screen appears, no session is selected and terminal input cannot reach A

#### Scenario: visible tab shortcuts

- GIVEN tabs belonging to multiple projects and one active project
- WHEN Command plus a digit from 1 through 9 is pressed
- THEN it selects the corresponding visible tab, or does nothing when that visible position is absent

#### Scenario: session creation completes after switching

- GIVEN a session launch for A is pending and B becomes active
- WHEN A's launch completes
- THEN its tab is retained for A without replacing B's selected terminal

Evidence: `apps/rune/src/panes.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/main.rs`

### RNC-project-workspaces

The system SHALL persist named workspaces with exactly one membership per registered project, migrate existing projects into a permanent Default workspace, and let the user select, create, rename, remove, and move projects between workspaces from the sidebar without changing project files, running sessions, tabs, or the editor checkout.

#### Scenario: migrate and restore

- GIVEN an existing database of registered projects and sessions
- WHEN the app upgrades and opens
- THEN Default contains every existing project with the same ids and session associations; a previously selected valid workspace is restored, otherwise Default is selected

#### Scenario: create and move

- GIVEN Default is selected and a project is registered there
- WHEN a named workspace is created and the project is moved to it
- THEN the new workspace is selected on creation and the project appears only in the destination after the move succeeds

#### Scenario: remove a workspace

- GIVEN a named workspace contains projects and may be selected
- WHEN its removal is confirmed
- THEN its projects move to Default atomically, Default becomes selected if necessary, and project files and running sessions remain untouched

#### Scenario: invalid or failed mutation

- GIVEN a blank or case-insensitive duplicate name, a missing id, or a storage failure
- WHEN the mutation is submitted
- THEN an error is shown and the existing workspace selection and membership remain intact

Evidence: `crates/app-core/src/store.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`

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

### RNC-session-account-label

The system SHALL label every Claude/Codex session row with the account it
runs under, derived from the session's own launch argv
(`account_name_of`): a `/usr/bin/env CLAUDE_CONFIG_DIR=…`/`CODEX_HOME=…`
word names the account whose folder matches (or the folder's basename when
the account was removed), and plain argv names the Default account —
independent of any later active-account switch.

#### Scenario: named-account session

- GIVEN a session whose argv starts `env CLAUDE_CONFIG_DIR=/accts/work`
- WHEN the sidebar renders its row
- THEN the label reads `Claude · work`

#### Scenario: removed-account session

- GIVEN a session whose argv names a folder no stored account owns
- WHEN the sidebar renders its row
- THEN the label shows the folder's last component

Evidence: `apps/rune/src/data.rs`, `apps/rune/src/sidebar.rs`

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

### RNC-settings-page

The system SHALL render the Settings view as a section rail (Editor,
Appearance, Accounts, ACP agents, Updates) beside the active section instead
of a placeholder: Editor exposes the native indentation preference (auto/2/4/8
spaces/tabs, persisted under `goko.editor.indent`); Appearance offers
System, Dark+, Light, One Dark, Catppuccin Mocha and Dracula persisted
under `goko.theme` with `Ink::for_theme` palettes and System following the
OS appearance; Accounts lists both agents' accounts with add (name +
folder), rename, remove (metadata-only — the folder and credentials stay)
and set-active actions, a sign-in action for the named accounts only (the
Default account has none — `accounts::sign_in_launch` refuses it, its login
belongs to a terminal), and a successful add auto-requests the agent's
sign-in session; the add and rename forms keep their drafts until the
matching worker reply arrives — a failed or refused submission shows the
reason inline and never loses what was typed — and a stale reply never
clears the form in front of the user; ACP agents expose each preset's
enabled flag plus command/prompt drafts saved together through `agents_set`
on the ACP runtime, and Reset removes that agent's stored override
(`acp::agents_reset`) so the preset is restored instead of being recreated
by the next store read; Updates names the running version, looks for a newer
release with a build for this machine (`latest.json` of the latest GitHub release, at startup, every six
hours and on "Check for updates"; `GOKO_NO_UPDATE_CHECK` stops the automatic
one) and, when one is out, says so, raises a notice toast once per version and
offers the install command to copy. No Vim control is rendered — it is deferred by
decision D1. Every mutation rides a worker lane (backend `Req`s or the ACP
runtime), never the UI thread.

#### Scenario: persist a theme

- GIVEN the Appearance section
- WHEN "Dracula" is chosen
- THEN the palette switches immediately and `goko.theme` stores `dracula`,
  restored on the next launch

#### Scenario: add an account

- GIVEN the Accounts section's add form with a name and folder
- WHEN Add is pressed
- THEN the account is created, activated, and its sign-in session is
  requested automatically

#### Scenario: a refused account

- GIVEN the add form holds a name and folder
- WHEN the worker rejects the add (a taken name, a refused queue)
- THEN the form keeps its name and folder and shows the reason inline

#### Scenario: sign in a named account

- GIVEN a named account row
- WHEN Sign in is pressed
- THEN the agent's login session starts in the account's folder

#### Scenario: remove an account

- GIVEN a named account's Remove with confirmation
- WHEN it is confirmed
- THEN only the stored row is deleted — the account folder stays on disk

#### Scenario: reset an ACP agent

- GIVEN an ACP agent's command was customized and saved
- WHEN Reset is pressed
- THEN the stored override is removed and the preset is effective again

#### Scenario: a newer release

- GIVEN the Updates section is open
- WHEN the check finds a release newer than the running version
- THEN the section shows the running version, says the newer one is available and offers "Copy install command"; a failed automatic check stays silent, one the user asked for shows its reason

Evidence: `apps/rune/src/settings.rs`, `apps/rune/src/update.rs`, `apps/rune/src/theme.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/acp.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/accounts.rs`, `crates/app-core/src/acp.rs`

### RNC-settings-toggle

The system SHALL toggle `State::view` between `Settings` and `Terminal` when
the header settings button is pressed; `Settings` replaces the whole workspace
column (tab strip, controls, content) with the native settings page —
Editor/Appearance/Accounts/ACP agents/Updates sections — matching App.tsx's
settingsMode. The press routes through the editor's draft guard like the view
switch, so leaving the Editor with unsaved work raises Save/Discard/Cancel
before the view changes. The button is a keyboard-focusable toggle named
"Settings" with a tooltip, pressed (`checked`) while `view` is `Settings`; it
shows that state as an accent tint with the gear turned one 60° notch (a
spring, instant under Reduce Motion), and gives hover wash, press wash and a
0.95 press scale as feedback.

#### Scenario: open settings

- GIVEN `view` is `Terminal`
- WHEN the settings button is pressed
- THEN `view` becomes `Settings` and the workspace column shows the settings page

#### Scenario: leave settings

- GIVEN `view` is `Settings`
- WHEN the settings button is pressed again
- THEN `view` returns to `Terminal`

#### Scenario: guarded press from a dirty editor

- GIVEN `view` is `Editor` and the open file has unsaved changes
- WHEN the settings button is pressed and the user chooses Cancel
- THEN `view` stays `Editor` with the draft intact

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/navigation.rs`

### RNC-sidebar-accessible-names

The system SHALL give every sidebar button an accessible name that includes its target: disclosure buttons read "Collapse <name>" or "Expand <name>", and row actions read "New session in <row name>", "More actions for <row name>", "Stop session <title>", and "Close pane <title>", with " (2)", " (3)" appended to repeated names so none is shared.

#### Scenario: no unnamed or duplicate names

- GIVEN a sidebar with several projects, a worktree group, and a session
- WHEN the accessibility tree is read
- THEN no sidebar button has an empty name and no two action buttons share a name

Evidence: `apps/rune/src/sidebar.rs`

### RNC-sidebar-projects

The system SHALL render the selected workspace's registered projects and per-project worktree lists from the editor backend — never mock rows — with one expandable project row (chevron, folder icon, name, missing badge, checkout status, branch label), a collapsible Worktrees group for linked checkouts, real daemon sessions nested under matching checkouts subject to the active-project filter in [RNC-project-terminals](#rnc-project-terminals), an Other sessions group for project sessions matching no checkout, honest loading/error/empty-group states, and Add Project through the native folder picker; Unassigned sessions appear only in Default when no project is active, while switching workspaces leaves existing tabs, sessions, and editor checkout intact.

#### Scenario: selected workspace

- GIVEN projects in two workspaces with worktrees and live sessions
- WHEN one workspace is selected
- THEN its projects and worktrees render, with nested sessions subject to the active-project filter, including a per-project worktree failure under only that row and unmatched sessions under Other sessions; the other workspace's project rows are hidden without stopping sessions or changing the editor checkout

#### Scenario: Unassigned and empty group

- GIVEN unassigned sessions and a named workspace with no projects
- WHEN the named workspace is selected
- THEN it shows an empty-group hint and Add Project, without Unassigned; Default shows Unassigned when no project is active

#### Scenario: add or move project

- GIVEN a workspace is selected
- WHEN a new folder is registered or an existing project is explicitly moved there
- THEN its project row appears there after the storage write succeeds; re-adding an already registered folder keeps its existing membership and reports that workspace

#### Scenario: loading or storage failure

- GIVEN project or workspace data is loading or fails to load
- WHEN the sidebar renders
- THEN it shows the loading state or error instead of fabricating project rows

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/worktrees.rs`

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
### RNC-sidebar-row-actions

The system SHALL reveal per-row hover actions matching the desktop: project
and worktree rows offer a New-session menu (agent presets discovered through
`app_core::agents`, plus a custom command) that issues `session.create` in
that checkout and records session metadata, and a "more actions" menu
(Open Editor, Refresh worktrees, Create worktree, Remove worktree on
worktree rows; Open Editor, Create worktree, Refresh worktrees, Remove
project on project rows) routed through the editor backend's guarded lanes;
session rows offer Stop and Close pane mapped to `session.stop` /
`session.delete`. A custom command is split into shell words — a quoted
argument stays one word — and an empty or unparseable command is refused
with a notice instead of starting a session.

#### Scenario: spawn a session from a project row

- GIVEN a registered project row with its hover actions revealed
- WHEN New session → an agent is chosen
- THEN `session.create` runs with the agent's resolved argv in the project
  checkout, metadata is recorded, and the session appears nested under it

#### Scenario: a preset without accounts launches

- GIVEN the Shell, OpenCode or Devin preset is chosen
- WHEN the worker resolves its argv
- THEN it launches as the Default account — only Claude and Codex resolve a
  stored account, so no preset is refused as an unknown agent

#### Scenario: two launches at once

- GIVEN two rows ask for an agent before either answer arrives
- WHEN the answers land, in either order
- THEN each answer starts its own request's session in its own checkout

#### Scenario: a refused launch

- GIVEN the account folder was removed or the agent's binary disappeared
- WHEN the argv reply fails
- THEN the failure is shown as the notice and no session starts

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

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/agents.rs`, `crates/app-core/src/worktrees.rs`, `crates/app-core/src/projects.rs`
### RNC-sidebar-selection

The system SHALL keep sidebar project/worktree row presses and the editor's
checkout selection in sync — committing a registered project root or listed
worktree as the editor's project+checkout through
`PendingNav::SelectCheckout` and the shared draft guard so unsaved work
prompts Save/Discard/Cancel first, and recording the committed checkout as
`active_workspace`, the sidebar row highlight, only after the guard passes,
so a cancelled or failed navigation restores the previous project, checkout,
highlight, and draft.

#### Scenario: worktree row selects the exact checkout

- GIVEN the editor is committed to one project's root checkout
- WHEN a worktree row — of the same or another project — is pressed
- THEN `SelectCheckout` commits that exact registered path and
  `active_workspace` highlights the matching row

#### Scenario: root row commits, Editor activates on open

- GIVEN `view` is `Terminal`
- WHEN a project root row is pressed
- THEN the selection alone stays in Terminal with the root checkout committed;
  opening the Editor view then activates that same project+checkout

#### Scenario: dirty draft guards sidebar selection

- GIVEN the open file has unsaved changes
- WHEN a project or worktree row is pressed and the user chooses Cancel
- THEN project, checkout, workspace highlight, and draft are all unchanged;
  Discard or a landed Save commits the pending selection instead

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`

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

### RNC-tab-debug

The system SHALL offer a secondary-press tab menu with Copy Tab ID, Copy Goko Session ID, Copy Agent Session ID, and Copy Debug Info, resolving session actions against the target tab's focused pane, exposing agent IDs only when attributable to that session, and reporting approximate local process-tree RAM for the whole tab without blocking the UI.

#### Scenario: target tab and focused pane

- GIVEN a tab with multiple terminal panes
- WHEN its tab menu is opened
- THEN Tab ID identifies that tab, session copy actions identify its focused pane, and Debug Info includes every pane's Goko session ID and PID

#### Scenario: agent identity unavailable

- GIVEN a Claude, Codex, OpenCode, Devin, Antigravity, or Shell session
- WHEN the tab menu opens
- THEN an attributable agent conversation ID can be copied, otherwise the agent copy item is disabled with a reason; Shell has no agent conversation ID

#### Scenario: memory estimate

- GIVEN live sessions in a tab
- WHEN a worker sample is available
- THEN RAM sums unique local process RSS across its session trees, labels the value approximate, and excludes shared Goko UI/daemon and cloud memory; a missing sample is reported as unavailable rather than zero

Evidence: `apps/rune/src/tab_debug.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/session_debug.rs`

### RNC-terminal-attach

The system SHALL keep each session's terminal state in the daemon and attach
the Terminal view to the selected session: when the daemon supports it,
`terminal.subscribe {paint: true}` starts the stream with a first paint (an ANSI
rebuild of the retained scrollback, the screen, cursor and terminal modes,
sent as kind-1 frames marked `PAINT_SEQ`) followed by live kind-1 frames, all
parsed into the alacritty `Term` grid rendered as styled runs; a daemon without
that support is attached through the retained raw replay. Keyboard input
encodes to `terminal.write`, viewport changes send `terminal.resize`,
`terminal.lagged` or a session switch resubscribes, `terminal.unsubscribe`
runs on detach, and an exited session keeps its final grid with input
suppressed.

#### Scenario: attach and interact

- GIVEN a running session selected in the Terminal view
- WHEN output frames arrive
- THEN the grid renders them; typing sends bytes the session receives (echo
  returns through the same path)

#### Scenario: GUI restart with retained history

- GIVEN a daemon kept a session while its output exceeded the raw replay buffer
- WHEN the GUI attaches again
- THEN the grid shows the current screen and the retained scrollback, and live
  output continues from the paint without a gap or duplicate

#### Scenario: no viewers

- GIVEN a session has no subscribers
- WHEN it writes output
- THEN the daemon's emulator keeps consuming it, so a later attach paints the
  current state

#### Scenario: lagged subscriber recovers

- GIVEN the terminal subscriber fell behind
- WHEN the daemon sends `terminal.lagged`
- THEN the client unsubscribes and resubscribes, and the new stream starts from
  a fresh paint (or the retained raw buffer on a daemon without paint)

#### Scenario: compatible legacy daemon

- GIVEN the daemon predates first paint
- WHEN Rune opens a session
- THEN it attaches with the raw replay and sends no unsupported request

#### Scenario: switch sessions

- GIVEN session A attached
- WHEN session B is selected
- THEN A is unsubscribed, B subscribes, and B's paint or retained output and
  live output render

Evidence: `crates/daemon/src/terminal.rs`, `crates/daemon/src/pty.rs`, `crates/daemon/src/server.rs`, `crates/daemon/tests/paint.rs`, `apps/rune/src/term.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/panes.rs`, `crates/protocol/src/frame.rs`, `crates/protocol/src/message.rs`

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

### RNC-terminal-wheel

The system SHALL route terminal wheel input according to the program's mouse and
alternate-screen modes, sending PTY wheel reports or alternate-scroll keys when
requested and otherwise scrolling terminal history, with Shift selecting history
instead of application reporting.

#### Scenario: primary history

- GIVEN primary-screen output has retained history and no mouse capture applies
- WHEN the user wheels upward
- THEN the view reveals older retained rows

#### Scenario: mouse reporting

- GIVEN the application requested terminal mouse reports
- WHEN the user wheels without Shift
- THEN the PTY receives reports with the correct direction, modifiers, coordinate encoding and bounds

#### Scenario: alternate scrolling

- GIVEN alternate screen and alternate-scroll are enabled without mouse capture
- WHEN the user wheels without Shift
- THEN the PTY receives correctly encoded Up/Down input rather than fabricated alternate-screen history

#### Scenario: explicit history override

- GIVEN a terminal application captures mouse or alternate scrolling
- WHEN the user wheels with Shift
- THEN the operation targets emulator history and emits no application wheel report; alternate-screen history may be empty

Evidence: `apps/rune/src/term.rs`, `apps/rune/src/panes.rs`

### RNC-terminal-splits

The system SHALL let a workspace tab split into an arbitrary tree of
terminal panes: each pane attaches its own alacritty `Terminal`
(subscribe/replay/resize/ack lifecycle per attachment), the active pane
receives input and the controls strip, separators drag or arrow-key the
split ratio clamped to measured pane minimums, and closing a pane drives
its stop→delete→collapse lifecycle with a visible, retryable failure and
a force kill after the stop hangs.

#### Scenario: split a pane

- GIVEN a tab showing a session's pane
- WHEN "Split side by side" is pressed
- THEN a sibling pane splits the space, each side keeping its own terminal
  attachment and both rendering live output

#### Scenario: close a pane

- GIVEN a pane whose session is running
- WHEN its Close is pressed
- THEN `session.stop` is sent, the pane collapses on delete, and a failure
  stays visible on the pane until retried or force-killed

Evidence: `apps/rune/src/panes.rs`, `apps/rune/src/main.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/sessions.rs`

### RNC-usage-accounts

The system SHALL read the usage snapshot and the accounts view on the
editor backend worker roughly every five seconds (`Req::Usage` carrying the
previous snapshot, plus an idle `wake_after` so a quiet UI still refreshes),
store them in `Data::usage`/`Data::accounts` only on successful replies,
and render one header chip per provider that has any limit or at least two
accounts: each present window shows its `windowLabel` and `used_pct` —
muted "reset" once `resets_at` has passed, the rounded percentage in
muted/warning/danger tones otherwise — and the chip tooltip lists each
window's reset and "Updated <age|unknown>". With two or more accounts the
chip suffixes the active account's name and opens a menu listing the
accounts (checked on the active one, each with its own usage summary in the
same expiry-aware form as the chips — a window that already reset reads
"reset", not its last percentage) whose selects send `AccountSetActive`,
plus "Manage accounts…" which opens the Settings page's Accounts section.
Account mutations (`AccountAdd`, `AccountRename`, `AccountRemove`,
`AccountSetActive`) run on the worker, mutate the cached view only on success
and refresh the usage read; errors surface as the app notice.

#### Scenario: quota chips render and refresh

- GIVEN a usage snapshot with a `five_hour` limit at 42 %
- WHEN the view renders
- THEN the Claude chip shows `Claude 5h 42%` and a ~5 s later worker read
  refreshes it

#### Scenario: elapsed window

- GIVEN a limit whose `resets_at` is in the past
- WHEN the view renders
- THEN the window shows a muted "reset" instead of a percentage

#### Scenario: multiple accounts

- GIVEN the Claude accounts view holds two accounts
- WHEN the active account differs from the chip's current name
- THEN the chip shows `Claude · <active name>` and its menu can activate
  the other account or open Settings → Accounts

#### Scenario: failed mutation

- GIVEN an account remove whose worker job fails
- WHEN the reply arrives
- THEN the cached view is unchanged and the notice shows the error

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/data.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/usage.rs`, `crates/app-core/src/accounts.rs`

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

### RNC-worktree-create-form

The system SHALL open the New-worktree form as a modal layer that owns the
keyboard while open: name (auto-synced path basename and branch until
touched), editable path with a native folder picker, new-branch or
existing-branch modes with base selection, and an optional env-file copy
with a worker-scanned preview. Creation sends `Req::WorktreeCreate` —
`worktrees::create` then `env_copy` — where a copy failure rides inside
the reply's `env` payload as partial success, the env report renders under
the sidebar's Worktrees group with Retry copy and Dismiss, and selection
of the new checkout routes through the dirty-draft navigation guard.

#### Scenario: create with env copy

- GIVEN the form filled with a name, path, new branch and env source
- WHEN Create is pressed
- THEN the worktree is created, the chosen `.env*` files are copied, and
  the new checkout becomes the selected workspace

#### Scenario: partial env failure

- GIVEN an env source whose copy fails
- WHEN the reply arrives
- THEN the worktree still exists, the env notice reports the failure, and
  Retry copy re-runs only the copy

Evidence: `apps/rune/src/editor/worktree_form.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/worktrees.rs`


## Flow

Terminal branch identity follows the focused pane’s session cwd, without falling back to the editor checkout. One background Git read runs at a time; results for a previous target are discarded. The header badge hides on target changes and failed reads. A three-second timer refreshes while Terminal is active; pending reads are checked every 50 ms.

`main.rs`'s `view` composes `chrome::header` full-width, then a row of
`sidebar::sidebar`, `sidebar::separator`, and a workspace column. When
`State::view` is `Settings` the column is only a placeholder; otherwise it
holds `chrome::tab_bar` and the content area — `panes::terminal_workspace` for
`Terminal`, `editor::workspace::workspace` for `Editor`. The session controls
strip (`chrome::controls_bar`) renders only in the Terminal view, matching
App.tsx where session controls live inside the terminal workspace; the editor
has no session pane. Each chrome function reads `State` (`Data`, tab
list, selection, view) and `Ink` (palette) and returns an `El` element tree;
shared builders (`txt`, `ic`, `btn_sm`, `btn_icon`, `usage_chip`) live in
`theme.rs`. `setup` kicks `editor.refresh_projects()` once so the sidebar's
real data loads with the app.

The sidebar (`sidebar::sidebar`) reads the editor's project state —
`editor.projects`, `editor.sidebar_worktrees`, and its own disclosure maps —
never a fabricated list. Its project-root and worktree row presses call
`editor::navigation::navigate` with `PendingNav::SelectCheckout`, sharing the
draft guard and commit path with the editor's pickers; session-row and tab
presses only mutate `Data`/`State`.

Project Map opens from the sidebar button or plain `⌘K` in Terminal/Editor.
`project_map::rows` groups registered project IDs beneath their workspace IDs,
with case-insensitive substring matching of workspace/project names and root
paths. A workspace click uses `State::select_workspace`; a project click uses
`PendingNav::SelectMappedProject` through the shared draft guard. Its apply
step resolves the current root/owner, commits both selections, and reveals the
keyed sidebar row against the new layout. Both paths retain the current view;
project jumps preserve all terminal layouts and running sessions while displaying only the destination project’s tabs and restoring its remembered pane focus.

Project selection through the sidebar, editor picker or Project Map calls
`sync_selection` after the draft guard commits. `visible_tabs` filters every
pane by its session metadata’s project ID; the tab strip, numbered shortcuts,
terminal rendering, input target, split controls and output subscriptions use
that same scope. The sidebar keeps project navigation rows and worktrees
visible while filtering their session lists. A pending create for another
project retains its new tab without changing the visible project’s selection.
With no active project, the existing unfiltered view remains available.

Secondary-pressing a tab stores its stable ID and pointer position without
activating it. `tab_debug::menu_layer` resolves its focused pane for session
copy actions; Copy Debug Info includes all its panes. Selection writes through
`Tx::copy` and shows a copy notice. Escape or an outside press dismisses the
menu; a vanished or hidden target also clears it.

The existing two-second subagent worker scan reads process RSS alongside PID,
parent PID and command. `session_debug::sample` collects each live session's
tree plus attributed tools below shared Codex servers. The tab totals the union
of PIDs across panes. While its menu is open, that worker also inspects agent
processes' open session files with `lsof`; no process command or database read
runs on the UI thread.

## State changes

In-memory only for the chrome itself: `open_tabs`, `active_tab`,
`project_tabs` (project ID → remembered tab ID), `selected_session`, `active_workspace`, `view`, and the sidebar disclosure
maps (`expanded`, `worktree_groups`, `worktrees_open`) mutate on tab
press/close, New tab, Stop, Close pane, session-row presses, sidebar chevron
and project/worktree presses, the view switch, and the settings button.
`active_workspace` — the sidebar highlight — commits only inside the editor's
post-guard `select_checkout` on a sidebar selection, and clears when the
editor drops its checkout; session and tab presses resolve it through
`workspace_for_session` to `__unassigned__` for sessions whose cwd matches no registered checkout.
The chrome opens no daemon, socket, or database connection of its own; its
sidebar actions only enqueue jobs on the editor backend, which owns all
persistence and is documented in
[Rune editor](rune-editor.md#state-changes).

Project Map's open flag, query and keyboard cursor are in memory. Only the
existing selected-workspace setting is persisted; no recent-project history is
written. Cancel or failed Save never commits the destination workspace setting.

The tab menu target, process snapshot, agent identity results, and sample clock
are ephemeral. They do not alter the daemon protocol, agent launch commands,
agent configuration, or persisted session metadata.

## Side effects

None from the chrome's session/tab controls — they render daemon session data and
perform no I/O beyond the window. The sidebar is no longer inert, though: its
"Add Project" button sends `Req::AddProject` (a storage write) and its
project/worktree presses commit a checkout selection whose section reads run
Git and file jobs — all on the editor backend's worker, documented in
[Rune editor](rune-editor.md#side-effects).

Project Map delegates selected-workspace persistence to the existing worker
`SetSetting` path and checkout reads to the existing editor refresh path. It
sends no session create/stop/delete request and changes no workspace membership
or project file; the draft guard's explicit Save retains its existing write.

Tab diagnostics read process metadata and, for a PID-owned Devin lock, validate
its basename with a read-only lookup of `sessions.id` in that agent's adjacent
database. Antigravity presence locks require a matching conversation database.
Claude transcript and Codex rollout filenames can identify their owning
process's conversation. The diagnostics read no conversation bodies and write
only the clipboard when the user selects a copy action.

## Authorization & constraints

None for the chrome — a local prototype shell with no access checks or
validation. Editor operations enforce their own project/checkout validation
in `app-core`; see [Rune editor](rune-editor.md#authorization--constraints).

Project Map only selects registered IDs from the current caches and rechecks
project ownership after a guard delay. Existing app-core checkout validation
still applies. It owns keyboard, paste and IME fallback while open, is suppressed
by other modal owners/terminal composition, and is unavailable in Settings.

All adapters share the same menu: Claude, Codex, OpenCode, Devin, Antigravity,
and Shell. A PID-linked session file takes precedence over an explicit agent
launch/resume ID. Launch IDs carry a warning that switching conversations
inside the CLI can change them. Multiple candidate IDs disable agent copying.
Shared session databases, directory matches and creation times alone are not
used to guess an ID. In particular, a fresh OpenCode or Codex CLI using only a
shared store/server may expose no attributable ID; a known explicit resume ID
is still available. Shell has no conversation ID of its own.

RAM is approximate local RSS, displayed in decimal MB. Each PID is summed once
per tab, but pages shared between different processes may still be counted
more than once. Goko UI/daemon, shared agent servers and cloud memory are
excluded. Values older than six seconds and missing process samples are
unavailable rather than zero. Keyboard, paste and terminal IME fallback do not
reach the terminal while the menu owns input.

## Error paths

`controls_bar` returns an empty element when no session is selected (and the
whole strip is absent in the Editor view). Closing the active project’s last tab clears `selected_session` and shows its start screen while retaining the committed checkout highlight. Hidden tabs remain stored; `active_tab` has no valid index when the current project has no tab. `sync_selection` clears the selection when the active tab's
title matches no session. Stop no-ops on non-live states, and the split
buttons remain inert — real pane splits need a layout the milestone doesn't
have — so they render dimmed and disabled, with no hover, a tooltip, and no Tab
stop. Disconnection only dims Stop and shows the header pill. The sidebar
shows honest status rows — "Loading projects…", the `projects_error` text, or
the empty-registry hint — and never fabricates a project row; a per-project
worktree failure renders under that project's row only, and refused backend
enqueues surface queue-full errors on the projects/worktree state rather than
spinning forever. Editor-side navigation failures (dirty-draft guard, stale
replies, backend errors) are documented in
[Rune editor](rune-editor.md#error-paths).

Project Map distinguishes loading, failed reads, no registered projects, empty
workspace groups and no search results. Initial/unready data cannot activate a
row; Retry reuses project/workspace refresh requests. A vanished target or owner
shows a notice without changing checkout/workspace. Workspace persistence failure
keeps the in-memory selection and reports the existing queue/write error.

An unavailable process command, `lsof`, session file, or busy/unreadable Devin
database leaves the corresponding diagnostic unavailable; other copy actions
remain usable. A stale tab/session at selection time shows a notice instead of
copying a different target. Exited sessions cannot supply a current agent ID.

## Tests

`apps/rune` carries extensive inline `#[test]`/`#[tokio::test]` cases across
`src/` — the editor modules plus `view_tests` full-app scenes in `src/main.rs`
covering the view switch, chrome composition, the draft-guard modal, picker
layout, session-controls absence in the Editor view, and the sidebar→editor
scenes: real-project listing with sessions under Unassigned, root and
worktree row selection committing identity, dirty-guard Discard/Cancel,
status rows, and real-backend fixtures driving cross-project selection
including in-flight replies (the count grows per slice; `cargo test -p
goko-rune` is the current total). See
[Rune editor](rune-editor.md#tests) for the coverage inventory; no dedicated
chrome-only test module exists.

Project terminal runtime tests in `main.rs` cover sidebar row/tab filtering,
worktree sessions, numbered visible-tab shortcuts, restoring a split’s focused
pane, no input target or controls for a project with no tab, editor-picker and
Project Map switches, late create replies, and deleting the current project’s
last tab while a foreign tab remains.

Project Map tests in `apps/rune/src/project_map.rs`, `main.rs`, and
`editor/navigation.rs` cover grouping/filtering, stable duplicate IDs, cursor
updates, data readiness, stale targets, guarded Save/Discard/Cancel and failed
writes, shortcut/input/paste/IME isolation, focus return and dismissal, distant
sidebar reveal and narrow-window bounds in light/dark themes.

`tab_debug.rs` tests copy payloads for the clicked tab and a split's focused
pane, every pane's debug data, unique RSS totals, stale samples and deleted
targets. Its Rune runtime test secondary-clicks an inactive tab, checks menu
entries, reads the copied ID from an in-memory clipboard and dismisses with
Escape. `session_debug.rs` tests process parsing, unique/missing memory,
launch IDs for all five agents, PID-separated open files, read-only Devin ID
validation, Antigravity presence metadata, ambiguous/exited identities and
attributed tools without charging the shared server.

## Related

- [Architecture overview](../architecture/overview.md)
- [Rune editor](rune-editor.md)

## Sources

- `apps/rune/src/terminal_branch.rs`

- `apps/rune/src/chrome.rs`
- `apps/rune/src/daemon.rs`
- `apps/rune/src/data.rs`
- `apps/rune/src/editor/acp.rs`
- `apps/rune/src/editor/backend.rs`
- `apps/rune/src/editor/backend/apply.rs`
- `apps/rune/src/editor/navigation.rs`
- `apps/rune/src/editor/state.rs`
- `apps/rune/src/editor/workspace.rs`
- `apps/rune/src/editor/worktree_form.rs`
- `apps/rune/src/main.rs`
- `apps/rune/src/panes.rs`
- `apps/rune/src/project_map.rs`
- `apps/rune/src/sessions.rs`
- `apps/rune/src/settings.rs`
- `apps/rune/src/sidebar.rs`
- `apps/rune/src/tab_debug.rs`
- `apps/rune/src/term.rs`
- `apps/rune/src/theme.rs`
- `crates/app-core/src/accounts.rs`
- `crates/app-core/src/acp.rs`
- `crates/app-core/src/agents.rs`
- `crates/app-core/src/projects.rs`
- `crates/app-core/src/session_debug.rs`
- `crates/app-core/src/store.rs`
- `crates/app-core/src/usage.rs`
- `crates/app-core/src/worktrees.rs`
- `crates/protocol/src/frame.rs`
- `crates/protocol/src/message.rs`

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
Appearance, Accounts, ACP agents) beside the active section instead of a
placeholder: Editor exposes the native indentation preference (auto/2/4/8
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
by the next store read. No Vim control is rendered — it is deferred by
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

Evidence: `apps/rune/src/settings.rs`, `apps/rune/src/theme.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/acp.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/accounts.rs`, `crates/app-core/src/acp.rs`

### RNC-settings-toggle

The system SHALL toggle `State::view` between `Settings` and `Terminal` when
the header settings button is pressed; `Settings` replaces the whole workspace
column (tab strip, controls, content) with the native settings page —
Editor/Appearance/Accounts/ACP agents sections — matching App.tsx's
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

### RNC-sidebar-projects

The system SHALL render the project sidebar from the editor backend's
registered projects and their per-project worktree lists — never a mock
session or project list — with one expandable row per project (chevron, folder icon,
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
- THEN it shows "Loading projects…", the error text, or the empty-registry
  hint — never a fabricated project row

#### Scenario: add a project

- GIVEN the sidebar header
- WHEN "Add Project" is pressed and a folder is chosen in the native dialog
- THEN `Req::AddProject` registers it through the editor backend and the
  refreshed project list renders the new row

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

## State changes

In-memory only for the chrome itself: `open_tabs`, `active_tab`,
`selected_session`, `active_workspace`, `view`, and the sidebar disclosure
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

## Side effects

None from the chrome's session/tab controls — they render daemon session data and
perform no I/O beyond the window. The sidebar is no longer inert, though: its
"Add Project" button sends `Req::AddProject` (a storage write) and its
project/worktree presses commit a checkout selection whose section reads run
Git and file jobs — all on the editor backend's worker, documented in
[Rune editor](rune-editor.md#side-effects).

## Authorization & constraints

None for the chrome — a local prototype shell with no access checks or
validation. Editor operations enforce their own project/checkout validation
in `app-core`; see [Rune editor](rune-editor.md#authorization--constraints).

## Error paths

`controls_bar` returns an empty element when no session is selected (and the
whole strip is absent in the Editor view). Closing the last tab clears
`selected_session`/`active_workspace`; `active_tab` re-clamps with
`saturating_sub`. `sync_selection` clears the selection when the active tab's
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

## Related

- [Architecture overview](../architecture/overview.md)
- [Rune editor](rune-editor.md)

## Sources

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
- `apps/rune/src/sessions.rs`
- `apps/rune/src/settings.rs`
- `apps/rune/src/sidebar.rs`
- `apps/rune/src/term.rs`
- `apps/rune/src/theme.rs`
- `crates/app-core/src/accounts.rs`
- `crates/app-core/src/acp.rs`
- `crates/app-core/src/agents.rs`
- `crates/app-core/src/projects.rs`
- `crates/app-core/src/store.rs`
- `crates/app-core/src/usage.rs`
- `crates/app-core/src/worktrees.rs`
- `crates/protocol/src/frame.rs`
- `crates/protocol/src/message.rs`

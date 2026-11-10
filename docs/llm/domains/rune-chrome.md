# Rune chrome

The `goko-rune` app (`apps/rune`, Rune/wgpu/winit) renders the Goko shell chrome —
the 48 px header, the project sidebar, the workspace tab strip, and the session
controls strip. Sessions and tabs stay mock data shaped like the protocol/Tauri
types: daemon and live-terminal wiring land in a later milestone, so sessions
never really run and the chrome's session buttons mutate the in-memory mock
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

### RNC-new-tab

The system SHALL create a workspace tab when New tab is pressed: it spawns a
mock running `shell` session via `data::spawn_shell` titled `Shell N`, pushes
the title onto `open_tabs`, makes it active, and selects the session.

#### Scenario: press New tab

- GIVEN `open_tabs` holds several titles
- WHEN New tab is pressed
- THEN a `Shell N` session is appended to `Data::sessions`, a matching tab opens and activates, and `selected_session`/`active_workspace` follow it

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`

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

### RNC-settings-toggle

The system SHALL toggle `State::view` between `Settings` and `Terminal` when
the header settings button is pressed; `Settings` replaces the whole workspace
column (tab strip, controls, content) with a placeholder, matching App.tsx's
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
- THEN `view` becomes `Settings` and the workspace column shows the settings placeholder

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
- THEN it shows "Loading projects…", the error text, or the empty-registry
  hint — never a fabricated project row

#### Scenario: add a project

- GIVEN the sidebar header
- WHEN "Add Project" is pressed and a folder is chosen in the native dialog
- THEN `Req::AddProject` registers it through the editor backend and the
  refreshed project list renders the new row

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/worktrees.rs`

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

## Flow

`main.rs`'s `view` composes `chrome::header` full-width, then a row of
`sidebar::sidebar`, `sidebar::separator`, and a workspace column. When
`State::view` is `Settings` the column is only a placeholder; otherwise it
holds `chrome::tab_bar` and the content area — `term::terminal_area` for
`Terminal`, `editor::workspace::workspace` for `Editor`. The session controls
strip (`chrome::controls_bar`) renders only in the Terminal view, matching
App.tsx where session controls live inside the terminal workspace; the editor
has no session pane. Each chrome function reads `State` (mock `Data`, tab
list, selection, view) and `Ink` (palette) and returns an `El` element tree;
shared builders (`txt`, `ic`, `btn_sm`, `btn_icon`, `usage_chip`) live in
`theme.rs`. `setup` kicks `editor.refresh_projects()` once so the sidebar's
real data loads with the app.

The sidebar (`sidebar::sidebar`) reads the editor's project state —
`editor.projects`, `editor.sidebar_worktrees`, and its own disclosure maps —
never mock `Data::projects`. Its project-root and worktree row presses call
`editor::navigation::navigate` with `PendingNav::SelectCheckout`, sharing the
draft guard and commit path with the editor's pickers; session-row and tab
presses still mutate only mock `Data`/`State`.

## State changes

In-memory only for the chrome itself: `open_tabs`, `active_tab`,
`selected_session`, `active_workspace`, `view`, and the sidebar disclosure
maps (`expanded`, `worktree_groups`, `worktrees_open`) mutate on tab
press/close, New tab, Stop, Close pane, session-row presses, sidebar chevron
and project/worktree presses, the view switch, and the settings button.
`active_workspace` — the sidebar highlight — commits only inside the editor's
post-guard `select_checkout` on a sidebar selection, and clears when the
editor drops its checkout; session and tab presses resolve it through
`workspace_for_session` to `__unassigned__` for every current mock session.
The chrome opens no daemon, socket, or database connection of its own; its
sidebar actions only enqueue jobs on the editor backend, which owns all
persistence and is documented in
[Rune editor](rune-editor.md#state-changes).

## Side effects

None from the chrome's session/tab controls — they render mock data and
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
scenes: real-project listing with mock sessions under Unassigned, root and
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
- `apps/rune/src/theme.rs`
- `apps/rune/src/data.rs`
- `apps/rune/src/sidebar.rs`
- `apps/rune/src/main.rs`
- `apps/rune/src/editor/state.rs`
- `apps/rune/src/editor/backend.rs`
- `apps/rune/src/editor/backend/apply.rs`
- `apps/rune/src/editor/workspace.rs`
- `apps/rune/src/editor/navigation.rs`
- `crates/app-core/src/projects.rs`
- `crates/app-core/src/worktrees.rs`

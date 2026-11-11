# Delta

Merged into docs/llm/: 2026-11-11

## ADDED Requirements

### RNC-project-workspaces

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; no existing requirement covers persistent project groups.

Requirement:

````md
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
````

## MODIFIED Requirements

### RNC-sidebar-projects

Previously: rendered all registered projects and Unassigned sessions together in one sidebar list.

Baseline:

````md
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
````

Replacement:

````md
### RNC-sidebar-projects

The system SHALL render the selected workspace's registered projects and per-project worktree lists from the editor backend — never mock rows — with one expandable project row (chevron, folder icon, name, missing badge, checkout status, branch label), a collapsible Worktrees group for linked checkouts, real daemon sessions nested under matching checkouts, an Other sessions group for project sessions matching no checkout, honest loading/error/empty-group states, and Add Project through the native folder picker; Unassigned sessions appear only in Default, while switching workspaces leaves existing tabs, sessions, and editor checkout intact.

#### Scenario: selected workspace

- GIVEN projects in two workspaces with worktrees and live sessions
- WHEN one workspace is selected
- THEN its projects, worktrees, and nested sessions render with their existing row behavior, including a per-project worktree failure under only that row and unmatched sessions under Other sessions; the other workspace's project rows are hidden without stopping sessions or changing the editor checkout

#### Scenario: Unassigned and empty group

- GIVEN unassigned sessions and a named workspace with no projects
- WHEN the named workspace is selected
- THEN it shows an empty-group hint and Add Project, without Unassigned; Default shows Unassigned

#### Scenario: add or move project

- GIVEN a workspace is selected
- WHEN a new folder is registered or an existing project is explicitly moved there
- THEN its project row appears there after the storage write succeeds; re-adding an already registered folder keeps its existing membership and reports that workspace

#### Scenario: loading or storage failure

- GIVEN project or workspace data is loading or fails to load
- WHEN the sidebar renders
- THEN it shows the loading state or error instead of fabricating project rows

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/worktrees.rs`
````

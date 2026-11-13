# Project terminal filter delta

Approved: 2026-11-11 — user approved the bounded chat design with “implement đi”: show only the active project's sessions and terminal tabs, retain other projects' running sessions and restore their tabs on return.
Merged into docs/llm/: 2026-11-13

## ADDED Requirements

### RNC-project-terminals

Page: `docs/llm/domains/rune-chrome.md`
Search: searched all RNC requirements in rune-chrome.md and open deltas; existing tab, sidebar and project-map requirements do not define project-scoped terminal visibility. No retired ID matches.

Requirement:

````md
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
````

## MODIFIED Requirements

### RNC-app-shortcuts

Previously: tab numbering and sidebar session lists were unfiltered by active project.

Baseline:

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

Replacement:

````md
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
````

### RNC-sidebar-projects

Previously: tab numbering and sidebar session lists were unfiltered by active project.

Baseline:

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

Replacement:

````md
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
````

## Verification record

Baseline: `cargo test -p goko-rune` passed all 658 existing tests before source edits.

Implementation: global tabs/layouts remain stored; `visible_tabs` and `current_tab` scope rendering, shortcuts, keyboard targets, splits and attachments. `project_tabs` remembers a stable tab ID per project. Sidebar session lists are filtered locally, leaving workspace attention counts unchanged. Post-guard checkout/project commits synchronize terminal selection; metadata reload/removal also rechecks it. No daemon, protocol, storage schema, dependencies, settings or process lifecycle changes.

Regression checks: `cargo test -p goko-rune` passed 661 tests, including three new project-terminal runtime tests and the updated Project Map test. Independent read-only review covered panes.rs, main.rs and editor/navigation.rs; no findings. Direct review covered tab-bar/sidebar callers and exact delta/wiki blocks.

Documentation: merged the exact requirement/replacement blocks into rune-chrome.md, and synchronized the still-open Project Map requirement/design with the approved follow-up. The whole-wiki validator reports 160 pre-existing missing-path errors in unchanged legacy desktop pages (architecture/editor.md: 86, architecture/overview.md: 31, domains/desktop-settings.md: 19, domains/desktop-theme.md: 24). The changed rune-chrome.md has zero validator errors; its pre-existing new-tab/new-tab-menu slug warning remains. Per review-and-verify, this documentation gate prevents a passing final review or archive. Full diagnostics: `/tmp/goko-project-filter-wiki.log`.

Verification limit: native macOS app appearance was not inspected; behavior is exercised through Rune runtime scenes. No commit or push requested or performed.

Final code checks: `cargo test --workspace` passed 1,035 tests across 22 suites; `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` and `git diff --check` passed. The first full test attempt hit the unrelated daemon CWD-after-exit assertion; that test passed individually and the subsequent entire workspace run passed. No daemon source was changed.

Final review: code and scoped requirement checks pass; Status: fail — existing whole-wiki documentation gate only. The change remains open, without a merge marker or archive, while the validator reports those unchanged legacy-page errors. Complexity pass found no in-scope simplification. Sidebar/project-picker/Project Map switching, tab bar, visible-tab shortcuts, empty controls/input and late session placement are covered by Rune runtime tests; attachment scoping is also exercised by the existing panes tests and confirmed through `current_tab` → `sync_terminals`.

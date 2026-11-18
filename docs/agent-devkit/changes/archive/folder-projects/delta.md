# Delta

Merged into docs/llm/: 2026-11-18

## ADDED Requirements

### RNC-nested-repo-import

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; no existing requirement covers scanning a non-Git folder for nested repositories, reviewing candidates, or importing them as a group, separately, or as a folder.

Requirement:

````md
### RNC-nested-repo-import

The system SHALL, when Add Project selects a folder that is not a non-bare Git worktree root, scan it on a detached thread for nested non-bare Git worktree roots with bounded depth, repository count, directory count and time — skipping VCS metadata, hidden and heavy build directories, symlinks, and any directory inside an already-found repository — then show a modal review listing every candidate selected with a group-name field and the actions Import as group, Import separately and Open as Folder, revalidating each imported path on the worker for containment and worktree-root status and reporting imported, already-known and failed counts; a Git worktree root takes the existing direct Add Project path, and a plain folder with no candidates offers only Open as Folder.

#### Scenario: plain parent folder with nested repositories

- GIVEN a folder that is not a Git worktree root and contains two nested Git worktree roots
- WHEN the user chooses it in Add Project
- THEN a modal review lists both candidates selected with the folder name as the group name, and Import as group registers the folder as a folder project and both repositories as its child projects in the selected workspace

#### Scenario: import separately or open the folder

- GIVEN the same review is open
- WHEN the user chooses Import separately
- THEN both repositories are registered as top-level projects in the selected workspace and the folder is not registered; choosing Open as Folder instead registers only the folder as a folder project

#### Scenario: bounded scan and partial import

- GIVEN a folder whose scan reaches the repository, directory or time bound, or a selected path fails revalidation
- WHEN the scan finishes or the import runs
- THEN the review shows a truncation or timeout notice, successful repositories are registered, and failed or already-known paths are reported without aborting the others

#### Scenario: no candidates or a Git root

- GIVEN the selected folder is a Git worktree root, or is a plain folder with no nested Git worktree root
- WHEN Add Project selects it
- THEN a Git root is registered directly with no review, and a plain folder with no candidates offers Open as Folder or Cancel

Evidence: `crates/app-core/src/project_scan.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/store.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`
````

### RNE-folder-project

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers a registered non-Git folder project, its filesystem Files tree, or its unavailable Git features.

Requirement:

````md
### RNE-folder-project

The system SHALL treat a registered project whose root is not a non-bare Git worktree root as a folder project: its Files section is the default section and lists the folder's filesystem recursively — regular files and symlinks, excluding `.git` at every level and the scan's VCS, hidden and heavy build directories, never traversing a symlinked directory, with `/`-separated relative paths and a bounded list that reports truncation — while file open, edit, save, create, rename, trash, media preview, Quick Open and Servers work through the folder root as the checkout; Changes, History and Stashes show an honest "not a Git repository" state without issuing Git reads, and the branch badge, worktree controls, blame, change gutter, content search and concept search are unavailable with an honest message instead of a Git error.

#### Scenario: browse and edit a folder project

- GIVEN a folder project is selected
- WHEN the editor opens
- THEN Files is active, the tree shows the folder's files and child repository files with `.git` hidden, and a text file opens, edits and saves through the existing draft guard

#### Scenario: Git features stay honest

- GIVEN a folder project is selected
- WHEN the user opens Changes, History or Stashes, presses the blame chip, or types in the content search box
- THEN the section shows that the folder is not a Git repository, no Git request is issued for it, and no branch, worktree, blame, gutter or search result is fabricated

#### Scenario: bounded folder list

- GIVEN the folder contains more files than the list bound
- WHEN Files loads
- THEN the tree shows the first bounded set and a truncation notice, and the editor remains usable

Evidence: `crates/app-core/src/git_view.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/navigation.rs`
````

## MODIFIED Requirements

### RNC-sidebar-projects

Previously: one expandable project row per registered project, no parent/child project hierarchy, and Add Project registered a single folder.

Baseline:

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

Replacement:

````md
### RNC-sidebar-projects

The system SHALL render the selected workspace's registered projects and per-project worktree lists from the editor backend — never mock rows — with one expandable project row (chevron, folder icon, name, missing badge, checkout status, branch label), child repository projects of a folder project indented under their parent row and expanded by the parent's chevron, a collapsible Worktrees group for linked checkouts, real daemon sessions nested under matching checkouts subject to the active-project filter in [RNC-project-terminals](#rnc-project-terminals), an Other sessions group for project sessions matching no checkout, honest loading/error/empty-group states, Add Project through the native folder picker and the nested-repository review in [RNC-nested-repo-import](#rnc-nested-repo-import), and a Rescan repositories action on a folder project's More menu; Unassigned sessions appear only in Default when no project is active, while switching workspaces leaves existing tabs, sessions, and editor checkout intact.

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

#### Scenario: folder project with child repositories

- GIVEN a folder project with two child repository projects in the selected workspace
- WHEN its row is expanded or collapsed
- THEN both child rows render indented under it as full project rows and collapse with it, and pressing a child row selects that child's root checkout through the shared draft guard

#### Scenario: loading or storage failure

- GIVEN project or workspace data is loading or fails to load
- WHEN the sidebar renders
- THEN it shows the loading state or error instead of fabricating project rows

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/data.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/projects.rs`, `crates/app-core/src/store.rs`, `crates/app-core/src/worktrees.rs`
````

### RNC-project-map

Previously: a Workspace → Project map whose project rows had no parent/child hierarchy.

Baseline:

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

Replacement:

````md
### RNC-project-map

The system SHALL provide a searchable, keyboard-accessible Workspace → Project map from plain `⌘K` or a sidebar button in Terminal/Editor, with workspace-only selection preserving the checkout and project selection atomically choosing its current owning workspace and root checkout through the draft guard while preserving the app view, terminal tabs and sessions, and with a folder project's child repository projects shown indented under their parent project row.

#### Scenario: inspect and search membership

- GIVEN registered projects in Default and named workspaces, including duplicate project names and a folder project with two child repositories
- WHEN Project Map opens and a trimmed case-insensitive workspace name, project name or root path is entered
- THEN matching projects appear under their owning workspace headings with distinguishing paths, a workspace-name or parent-project match includes that group's projects, a child match keeps its parent heading, and current workspace/project markers remain distinct from the keyboard cursor

#### Scenario: select a workspace

- GIVEN the map shows a valid workspace heading
- WHEN the user clicks it or activates it with Enter
- THEN the map closes, the sidebar selects and remembers that workspace through the existing settings path, and the checkout, view, tabs and sessions stay unchanged

#### Scenario: jump to a project

- GIVEN a project belongs to another workspace and no dirty draft blocks selection
- WHEN its map row is clicked or activated with Enter
- THEN the map closes, its current owning workspace is selected, its root checkout becomes active, the sidebar reveals its row, the existing view remains unchanged, and terminal visibility and selection follow the active project as described in [RNC-project-terminals](#rnc-project-terminals)

#### Scenario: jump to a child repository

- GIVEN a folder project has two child repository projects and no dirty draft blocks selection
- WHEN one child row is clicked or activated with Enter
- THEN the map closes, the child's owning workspace and root checkout become active, and the sidebar reveals the child row under its parent

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

### RNC-project-workspaces

Previously: moving one project moved exactly that project, and removing a workspace moved its projects to Default without a parent/child hierarchy.

Baseline:

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

Replacement:

````md
### RNC-project-workspaces

The system SHALL persist named workspaces with exactly one membership per registered project, migrate existing projects into a permanent Default workspace, and let the user select, create, rename, remove, and move projects between workspaces from the sidebar without changing project files, running sessions, tabs, or the editor checkout; moving a folder project moves its child repository projects to the same workspace in one transaction, moving a child detaches it from its parent, and removing a workspace moves its parents and children to Default with their hierarchy preserved.

#### Scenario: migrate and restore

- GIVEN an existing database of registered projects and sessions
- WHEN the app upgrades and opens
- THEN Default contains every existing project with the same ids and session associations; a previously selected valid workspace is restored, otherwise Default is selected

#### Scenario: create and move

- GIVEN Default is selected and a project is registered there
- WHEN a named workspace is created and the project is moved to it
- THEN the new workspace is selected on creation and the project appears only in the destination after the move succeeds

#### Scenario: move a folder project or child

- GIVEN a folder project with two child repository projects
- WHEN the parent is moved to another workspace
- THEN the parent and both children appear only in the destination; when a child is moved instead, it appears in the destination as a root project and the parent keeps its other children

#### Scenario: remove a workspace

- GIVEN a named workspace contains projects and may be selected
- WHEN its removal is confirmed
- THEN its projects move to Default atomically, Default becomes selected if necessary, project files and running sessions remain untouched, and any parent/child hierarchy is preserved

#### Scenario: invalid or failed mutation

- GIVEN a blank or case-insensitive duplicate name, a missing id, or a storage failure
- WHEN the mutation is submitted
- THEN an error is shown and the existing workspace selection and membership remain intact

Evidence: `crates/app-core/src/store.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`
````

### RNC-remove-confirm

Previously: the project removal confirmation stated only that the registration is removed, its sessions become Unassigned, and its files stay.

Baseline:

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

Replacement:

````md
### RNC-remove-confirm

The system SHALL ask for confirmation in an inline sidebar row before Remove worktree and Remove project run: the worktree row names the folder, says whether it has uncommitted changes (refreshed when the row opens), and states how many sessions that have not ended run in it, while the project row states that only the registration is removed, its sessions become Unassigned, and its files stay — adding that a folder project's child repositories stay registered without the group — Cancel changes nothing, and running sessions are never stopped.

#### Scenario: confirm a dirty worktree removal

- GIVEN a worktree row whose `dirty` value is true with two running sessions attached
- WHEN Remove worktree is chosen
- THEN an inline row names the folder, shows the uncommitted-changes line and "2 sessions", and nothing is removed until Confirm is pressed

#### Scenario: confirm a folder project removal

- GIVEN a folder project with two child repository projects
- WHEN Remove project is chosen
- THEN the confirmation states that the two child repositories stay registered without the group and that files stay; Confirm removes only the folder's registration and leaves both child rows at the top level

#### Scenario: cancel

- GIVEN a removal confirmation row is open
- WHEN Cancel is pressed
- THEN no request is enqueued and the row closes

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/main.rs`
````

### RNE-section-views

Previously: the editor always defaulted to Changes and did not describe a folder project's Git-only sections.

Baseline:

````md
### RNE-section-views

The system SHALL present the editor as five sections — Files, Changes, History, Stashes, Servers — defaulting to Changes, render only the active section's view while retaining each section's state in `EditorState` across switches, and issue a section's reads on activation, checkout selection, or manual refresh rather than for hidden sections.

#### Scenario: Switch sections

- GIVEN a checkout is selected and Changes is active with loaded data
- WHEN the user presses the History tab
- THEN `section` becomes History, its reads issue through `refresh_active`, and only History's view renders; returning to Changes restores its retained selection and data from `EditorState`

#### Scenario: Hidden sections stay cold

- GIVEN only Changes has been activated
- WHEN the workspace renders and polls
- THEN no Files, History, Stashes, or Servers reads are issued until each section is first activated

Evidence: `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/lsp.rs`
````

Replacement:

````md
### RNE-section-views

The system SHALL present the editor as five sections — Files, Changes, History, Stashes, Servers — defaulting to Changes for a Git checkout and Files for a folder project, render only the active section's view while retaining each section's state in `EditorState` across switches, and issue a section's reads on activation, checkout selection, or manual refresh rather than for hidden sections, while a folder project's Changes, History and Stashes show the honest not-a-Git-repository state without issuing Git reads.

#### Scenario: Switch sections

- GIVEN a checkout is selected and Changes is active with loaded data
- WHEN the user presses the History tab
- THEN `section` becomes History, its reads issue through `refresh_active`, and only History's view renders; returning to Changes restores its retained selection and data from `EditorState`

#### Scenario: Folder project defaults to Files

- GIVEN a folder project is selected
- WHEN the editor opens
- THEN Files is active, and pressing Changes, History or Stashes shows the not-a-Git-repository state without a Git read

#### Scenario: Hidden sections stay cold

- GIVEN only Changes has been activated
- WHEN the workspace renders and polls
- THEN no Files, History, Stashes, or Servers reads are issued until each section is first activated

Evidence: `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/lsp.rs`
````

### RNE-checkout-selection

Previously: every checkout job was validated through the same project/checkout rules, which required a real Git worktree for Git jobs.

Baseline:

````md
### RNE-checkout-selection

The system SHALL let the user pick a registered project and one of its checkouts — the project root or a listed Git worktree — through the workspace pickers or the chrome sidebar's project and worktree rows, routing every selection through the draft-guarded `SelectCheckout`/`SelectProject` navigation with each committed checkout becoming the sidebar's highlighted `active_workspace`, and validating every checkout through `app-core` project/checkout rules while never deriving editor selection from terminal sessions.

#### Scenario: Pick project and worktree

- GIVEN registered projects exist, one with linked worktrees
- WHEN the user picks a project then a checkout
- THEN the editor loads that checkout's sections through the guarded navigation path and keys all reads by `project_id` + canonical path

#### Scenario: Pick from the sidebar

- GIVEN the chrome sidebar lists a registered project's worktree row
- WHEN the row is pressed
- THEN the same guarded `SelectCheckout` commits that exact project+checkout — the stored project root for a project row, the listed worktree path for a worktree row — and `active_workspace` highlights it

#### Scenario: Invalid checkout rejected

- GIVEN a checkout path that is not the project root or a listed worktree
- WHEN the backend validates it for a Git or file job
- THEN `projects::git_checkout`/`project_checkout` rejects it and no Git command runs

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/data.rs`, `crates/app-core/src/projects.rs`
````

Replacement:

````md
### RNE-checkout-selection

The system SHALL let the user pick a registered project and one of its checkouts — the project root or a listed Git worktree — through the workspace pickers or the chrome sidebar's project and worktree rows, routing every selection through the draft-guarded `SelectCheckout`/`SelectProject` navigation with each committed checkout becoming the sidebar's highlighted `active_workspace`, and validating every checkout through `app-core` project/checkout rules: file jobs accept the project root or a listed worktree without requiring a Git worktree, while Git jobs additionally require a real non-bare Git worktree root, and editor selection is never derived from terminal sessions.

#### Scenario: Pick project and worktree

- GIVEN registered projects exist, one with linked worktrees
- WHEN the user picks a project then a checkout
- THEN the editor loads that checkout's sections through the guarded navigation path and keys all reads by `project_id` + canonical path

#### Scenario: Pick from the sidebar

- GIVEN the chrome sidebar lists a registered project's worktree row
- WHEN the row is pressed
- THEN the same guarded `SelectCheckout` commits that exact project+checkout — the stored project root for a project row, the listed worktree path for a worktree row — and `active_workspace` highlights it

#### Scenario: Invalid checkout rejected

- GIVEN a checkout path that is not the project root or a listed worktree
- WHEN the backend validates it for a Git or file job
- THEN `projects::file_checkout`/`git_checkout` rejects it and no Git command runs

#### Scenario: Folder project file job vs Git job

- GIVEN a registered folder project whose root is not a Git worktree
- WHEN the editor issues a file job and a Git job for that root
- THEN the file job proceeds through `projects::file_checkout` and the Git job is rejected by `projects::git_checkout` with the existing not-a-Git-worktree error

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/data.rs`, `crates/app-core/src/projects.rs`
````

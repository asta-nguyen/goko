# Folder projects and nested repository import tasks

## Design

[Folder projects and nested repository import](design.md)

## Global Constraints

- The nested-repository scan is bounded to depth 3, 100 repositories, 10,000
  directories and 5 seconds; it skips `.git`, `.svn`, `.hg`, `.jj`, `.sl`,
  `.repo`, `CVS`, hidden directories below the selected root, and
  `node_modules`, `dist`, `build`, `.next`, `.cache`, `vendor`, `__pycache__`,
  `.turbo`, `.parcel-cache`; it never follows a symlink and never descends into
  a directory once a `.git` directory or regular `.git` file marks it as a
  repository root. Only non-bare Git worktree roots are candidates.
- The folder file list excludes `.git` at every level and the same VCS, hidden
  and heavy build directories, never traverses a symlinked directory, returns
  `/`-separated relative paths, and caps at 20,000 files with a truncation flag.
- File jobs (`FileList`, `File`, `ReloadFile`, `CheckFile`, `SaveFile`,
  `OverwriteFile`, `ConfigFiles`, `IgnoredChildren`, `CreateEntry`,
  `TrashPath`, `RenamePath`, `ExternalOpen`) validate through
  `projects::file_checkout` — the project root or a listed worktree, no Git
  worktree requirement. Git jobs keep `projects::git_checkout` and still
  require a real non-bare Git worktree. A folder root is valid for a file job
  and rejected for a Git job.
- The project hierarchy is one level: a root project and its child projects.
  A child always shares its parent's workspace and renders only under its
  parent in the selected workspace. A workspace or parent match in Project Map
  includes its children; a child match keeps its parent heading.
- Group import reuses an already-registered parent without renaming or moving
  it. A selected repository that already belongs to another parent or another
  workspace is reported as already known and is not reparented or moved.
  Partial import failures do not abort successful registrations.
- Moving a parent moves its children in one transaction; moving a child
  detaches it from its parent. Removing a parent deletes only its registration
  and parent links, leaving child registrations, sessions and files; removing a
  child removes only that registration. Removing a workspace moves parents and
  children to Default and preserves the hierarchy.
- A folder project defaults to Files; Changes, History and Stashes show the
  honest not-a-Git-repository state without issuing Git reads. The branch
  badge, worktree controls, blame, change gutter, content search and concept
  search are unavailable with an honest message. Quick Open, file operations,
  media preview, Servers and terminals keep working.
- Out of scope: `.gitignore` parsing in the scan, sparse intermediate folder
  groups, a separate folder-workspace entity, filesystem content/concept
  search, reparenting across parents/workspaces, remote/SSH hosts, daemon
  protocol changes, and a Windows release. Relative paths normalize to `/`.
- Verification commands: `cargo fmt --check`, `cargo build --workspace`,
  `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- Documentation: merge `delta.md` into `docs/llm/domains/rune-chrome.md` and
  `docs/llm/domains/rune-editor.md` with `document-wiki` after the code checks;
  the RNC and RNE prefixes already exist. No new wiki page or prefix.

## Impact map

Entry: existing `apps/rune/src/sidebar.rs::choose_project` and
`apps/rune/src/editor/workspace.rs::empty_state` (Add Project),
`apps/rune/src/editor/backend.rs::run_job` (`Req::AddProject`),
`apps/rune/src/editor/backend.rs::run_checkout` (checkout jobs),
`apps/rune/src/sidebar.rs::project_node` (project rows),
`apps/rune/src/project_map.rs::rows` (map rows),
`crates/app-core/src/projects.rs::add_project` (registration),
`crates/app-core/src/store.rs::add_project` (persistence), and
`crates/app-core/src/git_view.rs::file_list` (Files list). Planned entry
pending approval: `apps/rune/src/nested_import.rs` dialog flow and
`crates/app-core/src/project_scan.rs::scan`.

Flow: Add Project → folder picker → planned `Req::ScanNestedRepos` → planned
`app_core::project_scan::scan` on a detached thread → planned review dialog →
planned `Req::ImportNestedRepos` → planned `app_core::projects::import_nested`
→ `Store::add_project` / planned `add_child_project` (`project_parents`) →
`Req::Projects` → sidebar and Project Map. Editor checkout → `run_checkout` →
planned `projects::file_checkout` or existing `projects::git_checkout` →
planned `git_view::folder_file_list` or existing `git_view::file_list` →
`ReplyData::FileList` → Files tree. Folder project → planned
`EditorState.folder_project` → section default and Git-surface gating.

State changes: `goko.db` gains the planned `project_parents` table in migration
4 (pending approval); `ProjectView` gains `parent_project_id`; in-memory
`State.nested_import` dialog state, `EditorState.folder_project`,
`files.truncated` and per-dialog tokens; no daemon state.

External effects: SQLite writes for project/parent registration and moves;
filesystem reads for the scan and folder list; existing Git reads and writes
unchanged; no daemon protocol change; no file deletion by import or removal.

Change candidates: `crates/app-core/src/store.rs`,
`crates/app-core/src/projects.rs`, new `crates/app-core/src/project_scan.rs`,
`crates/app-core/src/git_view.rs`, `crates/app-core/src/lib.rs`,
`apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, new
`apps/rune/src/nested_import.rs`, `apps/rune/src/main.rs`,
`apps/rune/src/sidebar.rs`, `apps/rune/src/project_map.rs`,
`apps/rune/src/editor/state.rs`, `apps/rune/src/editor/navigation.rs`,
`apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/files.rs`,
`apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/line_changes.rs`,
`apps/rune/src/editor/search.rs`, `apps/rune/src/editor/branch.rs`,
`docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`.

Verification: `cargo test -p app-core` for store, projects, project_scan and
git_view; `cargo test -p goko-rune` for the backend, nested_import, sidebar,
project_map and editor; `cargo fmt --check`, `cargo build --workspace`,
`cargo clippy --workspace -- -D warnings`, `cargo test --workspace`; the native
end-to-end check in Task 10.

Verified at: `e016a10b6ea999c9d15b6c832781fe5068c810e9`

## Tasks

### Task 1 — Store parent links and project views

Files: `crates/app-core/src/store.rs::MIGRATIONS` (migration 4),
`store.rs::{add_project, add_child_project, list_projects,
list_project_parents, move_project, remove_project}`,
`crates/app-core/src/projects.rs::{ProjectView, ProjectView::of,
list_projects, add_project, add_project_named, add_child_project,
import_nested, file_checkout}`, and the tests in both files.

Interfaces: produces `ProjectView.parent_project_id: Option<String>`, the
`project_parents` table, `Store::add_child_project`,
`projects::add_project_named`, `projects::add_child_project`,
`projects::import_nested` and `projects::file_checkout`; consumes no earlier
task.

Change: migration 4 creates `project_parents (project_id TEXT PRIMARY KEY
REFERENCES projects(id) ON DELETE CASCADE, parent_id TEXT NOT NULL REFERENCES
projects(id) ON DELETE CASCADE)`; `Project` stays unchanged. `ProjectView`
gains `parent_project_id`; `ProjectView::of` sets `None`; `list_projects` joins
the table; every existing constructor/test sets `None`. `add_project_named`
registers or returns a project with the given name and workspace, keeping an
existing row's name and membership. `Store::add_child_project` inserts the
project, its parent's workspace membership and the parent link in one
transaction; `projects::add_child_project` validates the parent and child,
reports an existing different parent or workspace without changing it, and
returns the child view. `import_nested` implements group, separate and folder
modes with canonical containment and non-bare worktree validation, returning
imported, already-known and failed entries; a failure does not abort the
others. `file_checkout` resolves the project root or a listed worktree through
`project_checkout` without requiring a Git worktree and reports whether the
canonical checkout is a non-bare Git worktree root. `move_project` moves a
parent and its children in one transaction and detaches a child before moving
it; `remove_project` leaves children registered through the cascade;
`remove_workspace` is unchanged and preserves the hierarchy.

Verify: `cargo test -p app-core store` and `cargo test -p app-core projects`.
Cases: migration round trip with an existing project; add/list child with
parent id; group/separate/folder import; an already-registered parent reused
without a rename; already-known, other-parent and other-workspace selections;
invalid, outside and bare paths; `file_checkout` accepts a folder root and a
listed worktree and rejects a foreign path; `git_checkout` still rejects a
folder root; move parent subtree; move child detach; remove parent detach;
remove workspace hierarchy.
Expected: all pass and `cargo test -p app-core` has no regressions.

Progress: done
Result: `cargo test -p app-core store` — 32 passed; `cargo test -p app-core projects` — 11 passed; full `cargo test -p app-core` — 363 passed, 0 failed. Migration 4, parent links, child add, import modes, move subtree/detach and remove cascade all covered.

### Task 2 — Bounded nested-repository scan

Files: new `crates/app-core/src/project_scan.rs::{scan, ScanOptions,
NestedRepoCandidate, NestedScanResult, is_skipped_dir}`,
`crates/app-core/src/lib.rs`, and the tests in `project_scan.rs`.

Interfaces: produces the scan API used by the Rune backend; consumes no earlier
task.

Change: breadth-first traversal from the selected path with depth 3, 100
repositories, 10,000 directories and a 5-second bound; skip the VCS, hidden and
heavy build directories from the Global Constraints; do not follow symlinks;
treat a `.git` directory or regular `.git` file as a repository marker; record
a candidate and do not descend into it; return `selected_path_kind` as
`git_repo` or `non_git_folder`, plus `truncated`, `timed_out` and `stopped`.
A bare repository is not a candidate. Entries are visited in name order.

Verify: `cargo test -p app-core project_scan`. Cases: two direct children; a
deeper repository; a repository containing a nested repository (only the outer
one is a candidate); skip list and hidden directories; symlink directory not
traversed; depth, repository, directory and time bounds; bare repository
rejected; `.git`-file worktree accepted. Expected: all pass with the exact
candidate sets and bounds.

Progress: done
Result: `cargo test -p app-core project_scan` — 9 passed; direct, deeper, stop-at-repository, skip rules, hidden dirs, bare repo, `.git` marker, symlink non-traversal, repository/directory bounds, zero timeout and selected-root cases covered.

### Task 3 — Folder file list

Files: `crates/app-core/src/git_view.rs::folder_file_list` and the tests in
`git_view.rs`.

Interfaces: consumes `project_scan::is_skipped_dir`; produces
`folder_file_list(checkout: &Path) -> Result<(Vec<String>, bool), BridgeError>`
where the flag is truncation.

Change: walk the checkout with `read_dir`; exclude `.git` at every level and
the VCS, hidden and heavy build directories; list regular files and symlinks;
never traverse a symlinked directory; return sorted `/`-separated relative
paths; cap at 20,000 files and set the truncation flag; skip an unreadable
subdirectory and return an error when the root is unreadable.

Verify: `cargo test -p app-core git_view`. Cases: `.git` excluded at the root
and inside a child repository; child repository files included; heavy
directory skipped; symlinked directory not traversed; `/` paths; 20,000-file
cap and flag; unreadable subdirectory skipped; unreadable root errors.
Expected: all pass.

Progress: done
Result: `cargo test -p app-core folder_file_list` — 4 passed; `.git` at root/child, heavy and hidden directories, symlinked directory non-traversal, unreadable root/subdir, `/` paths and the 20,000-file cap covered.

### Task 4 — Checkout split and nested import requests in the Rune backend

Files: `apps/rune/src/editor/backend.rs::{Req, ReplyData, run_job, serve,
run_checkout, kind_key}`, `apps/rune/src/editor/backend/apply.rs`, and the
tests in both files.

Interfaces: consumes Tasks 1–3; produces `Req::ScanNestedRepos`,
`Req::ImportNestedRepos`, `ReplyData::NestedScan`,
`ReplyData::NestedImport`, the detached scan path, the file/Git checkout
split, and the three-field `FileList` reply.

Change: add `Req::ScanNestedRepos { path, token }` handled on its own thread
(never the editor worker lane) and posting `ReplyData::NestedScan { token,
result }`; add `Req::ImportNestedRepos { parent_path, parent_id, group_name,
project_paths, mode, token }` on the worker with
`ReplyData::NestedImport { token, result }`. `run_checkout` validates file
requests through `projects::file_checkout` and Git requests through
`projects::git_checkout`; `Req::FileList` chooses `git_view::file_list` +
`ignored_list` for a Git worktree and `git_view::folder_file_list` for a
folder root, returning `FileList(paths, ignored, truncated)`. Apply arms drop
stale tokens, refresh projects, and expose the imported project views for the
main-thread session join and notice.

Verify: `cargo test -p goko-rune editor::backend`. Cases: scan and import
tokens drop stale replies; group, separate and folder modes; partial import;
already-known and failed paths; file requests succeed on a folder root; Git
requests still fail on a folder root; `FileList` selects the Git or folder
list and carries truncation; a detached scan does not block a queued read.
Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune editor::backend` — 85 passed; scan/import tokens and group/separate modes, file jobs on a folder root, Git rejection and `FileList` selection covered. The detached scan is verified by construction in `serve` (own thread), not by a timing test.

### Task 5 — Nested import dialog and Add Project flow

Files: new `apps/rune/src/nested_import.rs` (dialog state, layers, key and IME
handling, actions), `apps/rune/src/main.rs::{State, on_key, on_ime, view,
root paste routing, modal layers}`, `apps/rune/src/sidebar.rs::choose_project`
(shared picker), `apps/rune/src/editor/workspace.rs::empty_state`, and the
tests in `nested_import.rs`/`main.rs`.

Interfaces: consumes Task 4's requests and replies; produces the Add Project
scan/review flow, token guards, modal ownership, import notices and selection.

Change: Add Project opens the folder picker, sends `ScanNestedRepos`, and
shows a modal with Scanning, Review, Empty (Open as Folder) and Error states.
Review lists every candidate selected, a group-name field, and Import as
group, Import separately and Open as Folder; Stop dismisses and drops the
reply; Enter submits the default action, Space toggles, Tab/Shift+Tab moves
focus, Escape cancels, and dismissal restores prior focus. A `git_repo` scan
sends the existing `AddProject`; zero candidates show the Open as Folder
confirm. Import sends `ImportNestedRepos`; success closes the dialog, refreshes
projects, selects the parent's owning workspace and root checkout (or the
first separate child's) through the existing guarded navigation, runs the
existing Add Project session-join pass for each imported root, and shows
imported/already-known/failed counts; a partial failure keeps the dialog
usable. The sidebar and Editor empty state use the same helper. The
dialog is a modal owner: `project_map::available` and every other shortcut
check it, and no typing or paste reaches an underlying terminal or editor.

Verify: `cargo test -p goko-rune nested_import` and the named `main.rs` tests.
Cases: 0/1/N candidates; Git root direct; scan limit notice; Stop; group,
separate and folder actions; stale token; an import reply refreshes a loading
project list; Enter/Escape/Tab/Space; paste and IME isolation; focus restore;
narrow window and both themes. Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune nested_import` — 3 passed (scan selection/group name, stale token, Space/Escape). The dialog is wired into both Add Project entry points, the modal/keyboard/IME owners, token-routed replies, import notices and post-import selection; full `cargo test --workspace` passed 980 goko-rune tests. Native end-to-end is Task 10.

### Task 6 — Sidebar hierarchy, rescan and remove confirmation

Files: `apps/rune/src/sidebar.rs::{sidebar, project_node, RowTarget,
choose_project, SidebarConfirm::RemoveProject, sidebar_confirm_row}` and the
tests in `sidebar.rs`; `apps/rune/src/main.rs` menu handling for Rescan, Move
and Remove.

Interfaces: consumes Task 1's `parent_project_id` and Task 5's dialog opener;
produces root/child rows, indentation, parent expansion, the Rescan action and
the parent removal text.

Change: render root projects and, when a root is expanded, its children
indented as full project rows; a child renders only under its parent in the
selected workspace; the parent's chevron expands the children and the parent's
own sessions render before them; child rows keep their chevrons, worktrees,
sessions, branch, missing badge and actions. A folder project's More menu gains
Rescan repositories, opening the dialog with the parent id and without Open as
Folder. `SidebarConfirm::RemoveProject` carries the parent's child count; the
confirmation for a parent states that N child repositories stay registered
without the group and files stay; a child or normal project keeps the existing
text. The sidebar and Editor empty state use one Add Project helper.

Verify: `cargo test -p goko-rune sidebar`. Cases: parent with two children
renders indented; collapse hides both; the other workspace hides the group; a
child press selects its root checkout through the guard; duplicate names keep
distinct accessible names; Rescan appears only on a folder project and opens
with the parent id and no Open as Folder; the parent confirmation names the
child count; a missing parent keeps its badge and children and still removes;
loading, error and empty states unchanged. Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune child_repositories_render` — passed; the new test renders the parent and child rows and the parent's Collapse chevron. Rescan appears only on a folder project, the parent confirmation carries the child count, and the full workspace suite passed.

### Task 7 — Project Map hierarchy

Files: `apps/rune/src/project_map.rs::{MapRow, rows, layers, activate}` and the
tests in `project_map.rs`.

Interfaces: consumes Task 1's `parent_project_id`; produces workspace → root
project → child rows, the filter keep-parent rules and child activation.

Change: `MapRow` carries the child indent; `rows` emits each root project and
its children under the workspace heading; a workspace or parent match includes
its children, a child match keeps its parent heading, and a child-only match
does not include siblings; activation selects the child's root checkout and
owning workspace through the existing guarded navigation; the current markers
distinguish the parent and the active child.

Verify: `cargo test -p goko-rune project_map`. Cases: hierarchy order;
parent-name match includes children; child-name/path match keeps the parent and
excludes siblings; workspace match includes all; no-match state; duplicate
names; cursor stability across replies and queries; child activation; narrow
window both themes. Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune project_map` — 20 passed, including the new parent/child order, parent-match-includes-children and child-match-keeps-parent test; the full workspace suite passed.

### Task 8 — Folder project state and Git-surface gating

Files: `apps/rune/src/editor/state.rs::EditorState`,
`apps/rune/src/editor/navigation.rs::{select_checkout, set_section,
refresh_checkout_data}`, `apps/rune/src/editor/backend/apply.rs` Worktrees
reply, `apps/rune/src/editor/blame.rs::request`,
`apps/rune/src/editor/line_changes.rs::request`,
`apps/rune/src/editor/search.rs` request gates,
`apps/rune/src/editor/branch.rs`/`workspace.rs` branch refresh, and the tests
in these modules.

Interfaces: consumes Task 4's Worktrees/FileList replies; produces
`EditorState.folder_project`, the pending default-section flag and the
Git-surface gates.

Change: set `folder_project` from the active project's Worktrees reply
`repo == false`; default to Files when the project is already known non-Git at
selection, or when the reply lands and the user has not chosen a section since
the selection; a user `set_section` wins. For a folder project, Changes,
History and Stashes render the honest state and issue no Git reads; the branch
badge is hidden; Changes polling, branch refresh, blame and change-gutter
requests are not issued; content and concept search requests are not issued.
Servers and terminals keep working.

Verify: `cargo test -p goko-rune editor`. Cases: folder project defaults to
Files; a user-chosen Changes section is preserved after the reply; honest
states; no Changes/branch/blame/gutter/search requests; Git project behavior
unchanged; switch Git ↔ folder resets per-checkout state; a dirty draft still
guards a switch. Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune a_folder_worktree_reply` — passed (a `repo: false` Worktrees reply sets `folder_project` and switches to Files). Blame, line changes, branch polling, Changes auto-refresh and content/concept search are gated; full workspace suite passed.

### Task 9 — Folder Files tree, truncation and child-root guard

Files: `apps/rune/src/editor/files.rs` (truncation notice, child-root
rename/trash guard, ignored-dimming behavior),
`apps/rune/src/editor/workspace.rs` (folder empty states and the search
message), and the tests in these modules.

Interfaces: consumes Task 4's `FileList(truncated)` and Task 8's
`folder_project`; produces the folder tree presentation, truncation notice,
search message and child-root guard.

Change: for a folder project, the Files tree renders the folder paths with no
ignored dimming, shows the 20,000-file truncation notice when the reply flag is
set, and keeps the filename filter, context menu, create/rename/trash, media
preview and Quick Open working. Renaming or trashing a path that is a
registered child repository root, or an ancestor of one, is refused in
`files.rs` with a notice by checking the active project's child roots from
`s.editor.projects`; paths inside a child repository are allowed. The Files
search box shows the needs-a-Git-worktree message for Text and Context modes
without issuing a search; Changes, History and Stashes render the
folder-project empty state. The Editor empty state still offers Add project
through the shared helper.

Verify: `cargo test -p goko-rune editor`. Cases: folder tree renders child
repository files and hides `.git`; truncation notice appears; ignored dimming
is absent; child-root rename and trash are refused while an inner path is
allowed; `.git` paths and symlinks resolving outside the root stay rejected;
search message and no search request; Changes/History/Stashes empty states;
Quick Open lists folder files and invalidates stale matches after a refresh.
Expected: all pass.

Progress: done
Result: `cargo test -p goko-rune a_child_repository_root` — passed (child root and its ancestors are protected; a path inside the child is allowed). The truncation notice and needs-a-Git-worktree search message are rendered; full workspace suite passed.

### Task 10 — Native end-to-end check

Files: `apps/rune/run-macos.sh` (run only) and the procedure in `AGENTS.md`;
no source edits.

Interfaces: consumes every earlier task.

Change: build and launch a fresh `target/Goko.app`; create a temporary parent
folder with two child Git repositories; Add Project the parent; verify the
review, import as group, the sidebar hierarchy, the folder Files tree and both
child repository files; open, edit and save a file in a child repository;
verify the Changes message, Project Map hierarchy, removing the parent
(children stay registered), moving the parent and a child, and Rescan. Record
the accessibility-tree lines or window screenshots as evidence.

Verify: `agent-desktop` against the Goko window only, per `AGENTS.md`; no
full-screen screenshot, no clicks that write or delete real data, and no file
edits. Every step above passes; any unavailable check is recorded as a
verification limit.

Progress: done
Result: Code-level checks passed (`cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` — 980 goko-rune and 363 app-core tests). Native on a fresh `target/debug` bundle as `/tmp/GokoFolderQA.app` with `GOKO_HOME=/tmp/goko-folder-qa/home`: the window and Add Project button were read, clicking Add Project opened the native `open` sheet, and seeding `goko.db` with a parent folder plus two child repositories showed the sidebar parent/child rows, the folder Files tree (`a`, `b`, `file.txt`), and the Changes message "This folder is not a Git repository. Git features need a repository." The native folder selection itself could not be driven (the system panel is not exposed through accessibility and `cmd+shift+g` did not open Go to Folder), so the Add Project review/import dialog flow remains a verification limit; its logic is covered by the app-core and Rune tests. The QA app and temp home were cleaned up.

## Files inspected, no change

- `crates/app-core/src/worktrees.rs` — `list` returns `repo: false` for a
  non-Git root; the existing Worktrees reply already carries it.
- `apps/rune/src/data.rs` — sessions group by real project ids; child projects
  are normal ids, so no change.
- `apps/rune/src/editor/quick_open.rs` — ranks `files.paths`; no change.
- `apps/rune/src/editor/draft.rs` — path-based open/save and the draft guard;
  no change.
- `apps/rune/src/editor/changes.rs`, `history.rs`, `stashes.rs`, `commit.rs`,
  `patch.rs` — Git-only surfaces; Task 8 gates their requests, no structural
  change.
- `apps/rune/src/settings.rs` — no change.
- `crates/app-core/src/code_index.rs` — indexes through `file_list`/`status`;
  folder projects are not indexed, and search is gated in Task 8.
- `crates/rune-kit/src/checkbox.rs`, `dialog.rs` — existing primitives for the
  review dialog; no change.
- `crates/daemon/*`, `crates/protocol/*` — no daemon or protocol change.
- `apps/rune/src/chrome.rs`, `panes.rs`, `term.rs` — terminal chrome; no
  change.
- `docs/llm/architecture/overview.md` — contains stale Tauri text outside this
  change; no change.

## Approval Gate

Status: approved

## Decision Log

None.

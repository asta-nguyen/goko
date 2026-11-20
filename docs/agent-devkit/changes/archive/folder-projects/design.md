# Folder projects and nested repository import

## Goal

Let a plain parent folder that contains Git repositories be opened in Goko as one
folder project: its Files tree shows the folder and the child repositories, and
the user can import selected child repositories as Git projects nested under it.
Nothing is registered, reparented or moved without the user's explicit choice in
a review dialog.

## Scope

In scope:

- Add Project on a folder that is not a non-bare Git worktree root scans nested
  Git worktree roots with bounded depth, repository count, directory count and
  time, then shows a modal review.
- Import as group registers the parent as a folder project and selected child
  repositories as projects nested under it; Import separately registers only
  the selected repositories; Open as Folder registers only the parent.
- A folder project (a registered project whose root is not a non-bare Git
  worktree root) gets a filesystem-backed Files tree, file operations, Quick
  Open, terminals and language servers; Git-only surfaces show an honest
  not-a-Git-repository state.
- Sidebar and Project Map show child repository projects indented under their
  parent folder project.
- Store migration for the parent relationship, plus move/remove/rescan
  semantics.
- The Rune chrome and Rune editor wiki requirements listed in `delta.md`.

Out of scope:

- Sparse intermediate folder groups (Orca's folder scopes for nested
  subfolders); v1 groups one level: the selected parent and its direct and
  deeper repositories listed flat under it.
- A separate folder-workspace entity: the parent folder project is the group.
- Parsing `.gitignore` during the scan; v1 uses a fixed VCS/hidden/heavy-dir
  skip list.
- Filesystem content or concept search for folder projects; Quick Open works
  over the folder file list, content search reports that it needs a Git
  worktree.
- Reparenting a project that already belongs to another parent or workspace.
- Remote/SSH hosts; Goko is local-only.
- Changing the daemon protocol, terminal sessions, or worktree discovery.
- A Windows release; the scan and folder list stay platform-neutral and
  normalize relative paths to `/`.

This request produces a plan only. Application implementation requires approval
of this `design.md`, `tasks.md` and `delta.md`.

## Behavior

### Add Project on a folder

- The folder picker result is scanned first. The scan runs on its own thread,
  not the editor worker lane, and is bounded to depth 3, 100 repositories,
  10,000 directories and 5 seconds.
- The scan skips VCS metadata directories (`.git`, `.svn`, `.hg`, `.jj`, `.sl`,
  `.repo`, `CVS`), hidden directories below the selected root, and heavy build
  directories (`node_modules`, `dist`, `build`, `.next`, `.cache`, `vendor`,
  `__pycache__`, `.turbo`, `.parcel-cache`). It never follows a symlink and
  never descends into a directory once a `.git` directory or regular file marks
  it as a repository root. Entries are visited breadth-first in name order.
- A candidate is a non-bare Git worktree root: a `.git` directory or regular
  `.git` file. A bare repository is not a candidate.
- If the selected path is itself a non-bare Git worktree root, the scan returns
  `git_repo`; the app sends the existing `AddProject` request and no review
  dialog appears.
- If the selected path is a plain folder with no candidates, a confirm dialog
  says the folder is not a Git repository, that Files, Quick Open and terminals
  will work but Git features will not, and offers Open as Folder (registers the
  parent through `AddProject`) or Cancel.
- If the selected path is a plain folder with candidates, the review dialog
  shows the path, the candidate count, a scrollable checklist with every
  candidate selected, a group-name field defaulting to the folder name, and:
  - Import as group (default): registers the parent as a folder project with
    the group name in the selected workspace, then registers each selected
    repository as a child project under it in the same workspace, and selects
    the parent's owning workspace and root checkout through the existing
    guarded navigation.
  - Import separately: registers each selected repository as a top-level
    project in the selected workspace, and selects the first imported one and
    its workspace; the parent is not registered.
  - Open as Folder: registers only the parent and selects it.
- The review is modal, blocks input to the surfaces behind it, traps Tab focus,
  toggles rows with Space, submits the default action with Enter, cancels with
  Escape or Cancel, and restores the previous focus when dismissed.
- While scanning, the dialog shows a spinner and a Stop action. Stop dismisses
  the dialog; the bounded scan finishes in the background and its reply is
  dropped by token.
- A truncated or timed-out scan shows a limit notice in the dialog; the user
  may import the candidates found.
- Import revalidates every selected path on the worker: the parent is an
  existing directory, each selected path canonicalizes inside it, and each is a
  non-bare Git worktree root. Paths that fail are reported as failed without
  aborting the others. Already-registered paths are reported as already known.
- If the parent is already registered, group mode reuses its row and keeps its
  name, workspace and parent; the group-name field does not rename it. Newly
  selected repositories attach under it; a selected repository that already
  belongs to another parent or workspace is not reparented or moved.
- The parent folder project's More menu offers Rescan repositories. It runs the
  same scan and review for the parent's root with the existing parent as the
  import target and without Open as Folder. Existing children are listed and
  reported as already known when selected.

### Folder project

- A registered project whose root is not a non-bare Git worktree root is a
  folder project. The sidebar shows a plain folder icon, no branch and no
  worktrees; sessions started in the folder work as today.
- The editor defaults to Files for a folder project. Files lists the folder's
  filesystem recursively:
  - Regular files and symlinks are listed; `.git` is excluded at every level,
    and the scan's VCS, hidden and heavy build directories are skipped.
  - A symlinked directory is listed but never traversed.
  - Relative paths are `/`-separated and sorted; the list is capped at 20,000
    files and a truncation notice is shown when the cap is reached.
  - Empty directories are not listed (the tree derives folders from file paths,
    as the Git tree does).
- File open, edit, save, create, rename, trash, media preview, the filename
  filter and Quick Open work through the folder root as the checkout. The
  existing `.git` and containment rules still apply.
- Renaming or trashing a path that is a registered child repository root, or an
  ancestor of one, is refused from the folder tree with a notice; paths inside
  a child repository are allowed and its own Git project sees the change.
- Changes, History and Stashes show "This folder is not a Git repository. Git
  features need a repository." and issue no Git read. The branch badge,
  worktree controls, blame column, change gutter, content search and concept
  search are unavailable; the search box shows an honest needs-a-Git-worktree
  message and Quick Open still works. Servers (language servers) and terminals
  keep working.
- Opening a child repository file from the parent tree keeps the parent project
  identity and checkout; the child repository project remains the place for its
  Git features.
- The folder project's Files state is keyed by its checkout key, so switching
  to a Git project and back keeps the existing draft guard and per-checkout
  reset behavior.

### Checkout validation

- Checkout-scoped file requests — FileList, File, ReloadFile, CheckFile,
  SaveFile, OverwriteFile, ConfigFiles, IgnoredChildren, CreateEntry,
  TrashPath, RenamePath and ExternalOpen — validate through a new
  `projects::file_checkout`: the project root or a listed Git worktree, without
  requiring a Git worktree. A folder project's root is accepted; a path outside
  the project's root or listed worktrees is rejected.
- Git requests keep `projects::git_checkout`: project root or listed worktree
  plus a real non-bare Git worktree. A folder project rejects them with the
  existing "the checkout is not a Git worktree" error, and the UI does not
  issue them.

### Sidebar and Project Map

- The sidebar renders root projects (`parent_project_id` is null) and, when a
  root is expanded, its child projects indented under it. Child rows are full
  project rows: chevron, folder or Git icon, name, missing badge, checkout
  status, branch, worktrees, sessions and row actions. The parent's chevron
  expands its children; the parent's own sessions, if any, render before them.
- A child renders only under its parent and only when both are in the selected
  workspace. The hierarchy is one level.
- The Project Map shows workspace → root project → child projects, indented.
  Search matches workspace, project name or root path; a workspace or parent
  match includes its children, a child match keeps its parent heading, and
  activating a child selects its root checkout and owning workspace through the
  existing guarded navigation.
- Add Project, the Editor empty state and the parent's Rescan action all use the
  scan/review flow.
- Re-adding a registered folder keeps its membership, parent and name.
- Moving a parent moves its children to the same workspace in one transaction.
  Moving a child detaches it from its parent and moves it to the destination.
  Removing a parent deletes only its registration and parent links; child
  registrations, sessions and files stay, and the children become roots.
  Removing a child removes only that registration. Removing a workspace moves
  its projects, parents and children, to Default and preserves the hierarchy.
- The Remove project confirmation for a parent states that its child
  repositories stay registered without the group and that files are untouched;
  a child or normal project keeps the existing text.

## Edge cases

- Scan path is a Git worktree root: no review; normal Add Project.
- Scan finds no candidates: Open as Folder confirm.
- Scan is truncated, times out or is stopped: limit notice; import only the
  candidates found; a stopped scan's reply is dropped by token.
- The parent is deleted, unreadable, or replaced between scan and import:
  import fails with a notice and registers nothing.
- A selected path escapes the parent through `..` or a symlink: rejected and
  reported as failed.
- A selected path is a bare repository or a plain directory: rejected and
  reported as failed.
- A selected repository is already registered under another parent or another
  workspace: reported as already known; it is not reparented or moved.
- The parent is already registered: reused; its name and workspace stay; new
  children attach.
- A parent link is removed while the child remains registered: the child
  becomes a root on the next project list.
- Moving a child: it detaches; the parent keeps its other children.
- A parent folder is missing: the parent row shows its missing badge, children
  still render, opening the parent fails honestly, and removal still works.
- Folder file list: an unreadable subdirectory is skipped; an unreadable root
  shows a Files error; reaching the cap shows the truncation notice; `.git` is
  never listed; a symlinked directory is never traversed.
- Folder file open: `.git` paths, symlinks resolving outside the root, binary,
  non-UTF-8 and oversized files follow the existing read-only and containment
  rules.
- Folder project Git surfaces: no Changes poll, branch refresh, blame or gutter
  request is issued; the existing per-project Worktrees read still runs to
  establish `repo: false`, and each Git surface shows the honest state instead.
- Quick Open after a folder list refresh or checkout switch: the existing stale
  match invalidation applies.
- Two scans or imports in flight: the token of the visible dialog is the only
  reply applied; the scan thread never blocks the editor worker lane.
- Import while the project list is loading: the existing loading state stays
  visible and the list refreshes after the import reply.
- A dirty draft while switching away from a folder project: the existing draft
  guard applies and Cancel preserves the draft. Moving or removing a project
  registration does not change the editor checkout, so an open draft stays as
  the existing behavior.
- Windows path separators: the scan and store keep canonical OS paths; the
  folder file list converts relative paths to `/`.

## Interfaces

- `crates/app-core/src/store.rs`: migration 4 creates
  `project_parents (project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE, parent_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE)`.
  Adds `list_project_parents`, `add_child_project`, and subtree-aware
  `move_project`; `remove_project` relies on the cascade to detach children;
  `remove_workspace` is unchanged.
- `crates/app-core/src/projects.rs`: `ProjectView.parent_project_id`,
  `list_projects` joins parents, `add_project_named`, `add_child_project`,
  `import_nested`, and `file_checkout` returning the canonical checkout plus
  whether it is a Git worktree root; `git_checkout` is unchanged.
- New `crates/app-core/src/project_scan.rs`: bounded nested-repository scan
  (`scan` with options, candidates, `truncated`/`timed_out`/`stopped`), shared
  skip rules, and candidate validation.
- `crates/app-core/src/git_view.rs`: `folder_file_list` walking the filesystem
  with the same skip rules, `/`-separated paths, a 20,000-file cap and a
  truncation flag. Existing `file`, `save_file`, `create_entry`, `rename_path`
  and `entry_path` remain path-based and keep their containment and `.git`
  rules.
- `apps/rune/src/editor/backend.rs`: new `Req::ScanNestedRepos { path, token }`
  and `Req::ImportNestedRepos { parent_path, parent_id, group_name,
  project_paths, mode, token }`; `ReplyData::NestedScan` and
  `ReplyData::NestedImport`; the scan is posted from a detached thread;
  `run_checkout` splits file requests through `file_checkout` and Git requests
  through `git_checkout`; `FileList` chooses the Git or folder list and carries
  `truncated`; `AddProject` remains for direct and cd-discovered adds.
- `apps/rune/src/editor/backend/apply.rs`: apply scan/import replies by token
  and refresh projects; `apps/rune/src/main.rs` runs the same session-join pass
  as Add Project for newly registered roots and surfaces partial-import counts.
- `apps/rune/src/main.rs`: `State.nested_import` dialog state (path, parent id,
  token, scan result, selection, group name, scanning/importing/error flags),
  modal layers, keyboard/paste routing, modal-owner checks, the Add Project
  entry points and the Rescan action.
- `apps/rune/src/sidebar.rs`: root/child project rendering and indentation,
  parent group expansion, Rescan menu item, parent removal confirmation, and
  the shared Add Project picker helper used by the Editor empty state.
- `apps/rune/src/project_map.rs`: root/child rows, filter keep-parent rules,
  activation unchanged.
- `apps/rune/src/editor/state.rs`: `folder_project` flag, `files.truncated`,
  and the pending default-section flag.
- `apps/rune/src/editor/navigation.rs`: default Files for a folder project
  after the worktree reply confirms `repo: false`; `select_checkout` keeps the
  current section when the user already chose one.
- `apps/rune/src/editor/workspace.rs`: folder-project empty states, hidden Git
  controls, and the search needs-a-Git-worktree message.
- `apps/rune/src/editor/files.rs`: truncation notice, no ignored dimming for a
  folder project, and the child-root rename/trash guard.
- `apps/rune/src/editor/blame.rs`, `line_changes.rs`, `search.rs`, `branch.rs`:
  gate requests on `folder_project`.
- `docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`: the
  requirements in `delta.md`.

## Verification

- app-core store: migration 4 round trip, child add/list, move parent subtree,
  move child detach, remove parent detach, remove workspace hierarchy, and the
  existing project tests.
- app-core scan: direct children, deeper candidates, stop at a repo root, skip
  rules, hidden and heavy dirs, symlink non-traversal, depth/repo/dir/time
  bounds, bare-repo rejection, and `.git`-file worktree roots.
- app-core folder list: `.git` excluded, child repo files included, heavy dirs
  skipped, symlinked dirs not traversed, `/` paths, cap and truncation, and
  unreadable directory handling.
- app-core checkout: `file_checkout` accepts a folder root and a listed
  worktree and rejects foreign paths; `git_checkout` still rejects a folder
  root.
- Rune backend: scan/import token staleness, group/separate/folder modes,
  already-known and failed paths, partial import, file requests on a folder
  project, Git requests still rejected, FileList Git/folder selection, and the
  truncation flag.
- Rune UI: Add Project review for 0/1/N candidates, Stop, scan limit notice,
  import actions and notices; sidebar child rows, collapse and indentation;
  Project Map hierarchy, filter keep-parent and child activation; folder Files
  tree, open/edit/save, Quick Open, and the child-root rename/trash guard; Git
  sections honest states with no Git requests; default Files; draft guard;
  move/remove/rescan semantics.
- Native check per `AGENTS.md`: create a temp parent with two child repos, run
  `apps/rune/run-macos.sh debug`, Add Project the parent, import as group,
  verify the sidebar hierarchy, the Files tree and child files, open/edit a
  child file, Changes message, Project Map, remove the parent (children stay),
  move the parent and a child, and rescan. Use `agent-desktop` on the Goko
  window only; no full-screen screenshot.
- Commands: `cargo fmt --check`, `cargo build --workspace`,
  `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.

## Related context

- [Rune chrome](../../../../llm/domains/rune-chrome.md)
- [Rune editor](../../../../llm/domains/rune-editor.md)

# Multi-project workspaces

## Goal

Let a user group related registered projects into named workspaces and show one group's projects in the Rune sidebar at a time, reducing sidebar clutter without interrupting open work.

## Scope

- Add persistent named workspaces above registered projects. Every project belongs to exactly one workspace. A permanent **Default** workspace receives all projects in existing databases.
- In the sidebar, select, create, rename, and remove a workspace; add a project to the selected workspace; move a project to another workspace from its row menu.
- Persist the selected workspace across app restarts. Keep the editor project picker global.
- Keep existing worktree, session, terminal tab, editor, and checkout behavior when switching workspaces.
- No daemon or protocol changes, per-workspace terminal tabs, duplicate project membership, project file operations, or visual redesign outside the sidebar.

## Behavior

- The sidebar header shows the selected workspace and a selector containing Default and all named workspaces. Selecting one filters project rows and their nested worktrees and sessions. Switching does not select a project, navigate the editor, close a tab, or stop a session. An active checkout may be outside the visible group; in that case no sidebar row is highlighted.
- **Create workspace** asks for a name, saves it, and selects the new empty workspace. **Rename** changes only a named workspace's label. **Remove workspace** requires confirmation, moves all its projects to Default in one storage transaction, then removes the workspace; if it was selected, Default becomes selected. Default cannot be renamed or removed. No project directory or running session is touched.
- Names are trimmed, nonempty, and unique ignoring case. A duplicate or failed mutation leaves the existing list and selected workspace intact and shows an error. The name entry stays available for correction after a failure.
- **Add Project** associates a newly registered directory with the selected workspace. Re-adding an already registered canonical directory leaves its membership unchanged and reports where it already belongs; moving is an explicit row action. A project's **Move to workspace** menu lists other workspaces and updates only its membership, with the row moving after a successful write. A failed move leaves it in its original workspace and shows an error.
- Unassigned sessions appear only in Default. Sessions associated with a project follow that project's workspace. The editor's global project picker can still reach every registered project. The sidebar's project/worktree selection retains its existing draft guard.
- On first launch or migration, Default exists. Existing project rows are assigned to it without changing ids, roots, session metadata, or checkout references. The most recently selected workspace is restored on launch; if it no longer exists, select Default. Storage read or migration errors remain visible in the sidebar and do not fabricate a workspace or project row.

## Edge cases

- Empty selected workspace shows an empty-group hint and Add Project; loading and storage errors remain distinct from empty.
- Concurrent or stale replies must not select a deleted workspace or overwrite a newer selection. On workspace deletion, transfer and delete succeed or fail together. A missing project id or workspace id is an error; no unrelated membership changes.
- Switching away from a group with the active editor checkout or running session leaves those open and working. Returning shows their existing rows and active highlight.
- Project removal retains its existing behavior: only its registration is deleted, sessions become Unassigned and appear in Default; project files and running sessions remain.

## Interfaces

- `crates/app-core/src/store.rs` migrates `goko.db` with a workspace table and project membership, and provides transactional workspace CRUD and project move. `crates/app-core/src/projects.rs` continues to validate project paths and exposes membership with project listings.
- `apps/rune/src/editor/backend.rs` and `backend/apply.rs` carry workspace reads and mutations on the existing project worker lane and refresh the sidebar after successful writes.
- `apps/rune/src/main.rs` owns the selected workspace and its persisted setting; `apps/rune/src/sidebar.rs` renders the selector, management actions, filtered rows, and error/empty states. Existing checkout `active_workspace` keeps its current meaning; the group selection uses a distinct field and setting.
- `docs/llm/domains/rune-chrome.md` records the resulting sidebar requirements after implementation. The existing editor selection requirements remain unchanged.

## Verification

- Store tests cover migration of an existing database, default assignment, name validation and uniqueness, create/rename/delete, transactional transfer to Default, explicit move, and unchanged project/session records.
- Rune tests cover sidebar filtering, Unassigned placement, workspace selection persistence and fallback, Add Project membership, duplicate registration, move, mutation failures, and unaffected tabs/editor selection when switching.
- Run focused tests during implementation, then `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, and `cargo clippy --workspace -- -D warnings`. Inspect the native sidebar through `apps/rune/run-macos.sh debug` when the app can launch in this environment.

## Related context

- [Rune chrome](../../../../llm/domains/rune-chrome.md)
- [Rune editor](../../../../llm/domains/rune-editor.md)

## Previous work

- [S2 projects, agents, and sessions](../../archive/2026-10-30-s2-projects-agents-sessions/design.md) established the single project tree, registration, and session metadata.
- [Rune live sessions](../../archive/rune-live-sessions/design.md) established the current native sidebar and session nesting.

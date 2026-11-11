# Multi-project workspaces tasks

## Design

[Multi-project workspaces](design.md)

## Global Constraints

Every project belongs to exactly one workspace. A permanent **Default** workspace receives all projects in existing databases. Switching does not select a project, navigate the editor, close a tab, or stop a session. **Remove workspace** requires confirmation, moves all its projects to Default in one storage transaction, then removes the workspace. No project directory or running session is touched. Existing checkout `active_workspace` keeps its current meaning; the group selection uses a distinct field and setting.

## Impact map

Entry: `apps/rune/src/sidebar.rs::sidebar`, `add_project`, `menu_layer`; `apps/rune/src/main.rs::State::apply_reply` and startup setup.
Flow: sidebar action → `editor/backend.rs::Req` on project worker lane → `app-core::store::Store` / `projects` → reply application → sidebar state/render; project/worktree selection → `editor/navigation.rs` draft guard.
State changes: `crates/app-core/src/store.rs` SQLite `projects`, `sessions`, `settings`; planned workspace table and project membership; planned selected-workspace setting.
External effects: `goko.db` migration and writes; native folder picker for Add Project. Daemon sessions and project directories remain unchanged.
Change candidates: `crates/app-core/src/store.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sidebar.rs`, their direct test fixtures, `docs/llm/domains/rune-chrome.md`.
Verification: focused store and Rune tests, workspace Cargo checks, native sidebar inspection when launch is available.
Verified at: 93f5cc1bc5028bc7f0e6e17ca8cfd3659acbecdd

## Tasks

### Task 1 — Persist workspace membership safely

Files: `crates/app-core/src/store.rs::MIGRATIONS`, `Store`; `crates/app-core/src/projects.rs::list_projects`, `add_project`; store and project tests.
Interfaces: Produce persistent Default and named workspace rows, project membership in listings, and storage operations for create, rename, remove with atomic transfer, and explicit project move. Existing project and session ids stay stable.
Change: Migrate existing databases to assign every project to Default; validate trimmed nonempty case-insensitive unique names and missing ids; keep duplicate project registration in its original workspace. Keep writes transactional where membership and workspace deletion must agree.
Verify: Existing-db migration → all old projects in Default with unchanged session associations; create/rename/duplicate/invalid name cases; move → only target membership changes; delete → all member projects in Default and no project/session deletion; forced write failure → transaction rolls back.
Progress: done
Result: `cargo test -p app-core` (246 passed) — migration test assigns every old project to Default with session associations unchanged; create/rename/duplicate/invalid-name, move-only-membership, atomic remove-with-transfer (forced failure rolls back), duplicate-root keeps membership, cascade on project delete.

### Task 2 — Route workspace jobs and restore selection

Files: `apps/rune/src/editor/backend.rs::Req`, `ReplyData`, `run_job`; `apps/rune/src/editor/backend/apply.rs::EditorState::apply_reply`; `apps/rune/src/editor/state.rs::EditorState`; `apps/rune/src/main.rs::State`, `State::apply_reply`, startup setup; existing backend and main tests.
Interfaces: Consume Task 1 storage operations; expose workspace list and mutation replies through the project worker lane; produce a distinct selected group and persisted setting for the sidebar. Preserve the existing checkout `active_workspace` key.
Change: Load workspaces and projects on startup; restore last selected workspace only if present, otherwise Default. Refresh membership after successful add/move/remove/rename; keep selection and form data on failed writes; reject stale replies that could restore a deleted or superseded selection. Keep Add Project's no-auto-navigation behavior.
Verify: New and migrated homes select Default; saved named workspace restores; missing saved id falls back; mutation error retains selection; out-of-order replies do not override newer selection; existing project/checkout selection tests still pass.
Progress: done
Result: `cargo test -p goko-rune` — workspace reply selects Default, saved id restores, missing id falls back, stale restore cannot override a newer selection, create/remove replies select correctly, failed create/rename keeps prompt and selection; all existing project/checkout selection tests still pass.

### Task 3 — Render and operate workspace sidebar

Files: `apps/rune/src/sidebar.rs::sidebar`, `add_project`, `menu_layer`, `SidebarInput`; `apps/rune/src/main.rs::State`; sidebar and main view tests.
Interfaces: Consume Task 2 workspace state/jobs; expose selector, create/rename/remove controls and project move action with existing Rune menu/input patterns.
Change: Filter project rows and nested sessions by selected workspace; show Unassigned only in Default; show distinct loading/error/empty-group states; create selects its new workspace, remove confirms transfer and selects Default when needed; add registers into selected workspace, duplicate reports its existing group, move updates membership after success. Preserve open tabs and editor checkout on group switch.
Verify: Two groups show only their member projects and sessions; Unassigned only in Default; active session/editor in hidden group keeps running/selected; add, move, rename, delete, duplicate and failed mutation render correctly; accessibility names and keyboard focus for selector/actions.
Progress: done
Result: `cargo test -p goko-rune` — sidebar filters by selected workspace, Unassigned only in Default, empty named group hint, switching keeps editor checkout/tabs/sessions, selector and removal confirmation expose button roles/labels/focus, duplicate add reports its workspace.

### Task 4 — Integrate and document

Files: `docs/llm/domains/rune-chrome.md::RNC-sidebar-projects`, new workspace requirement; `docs/llm/INDEX.md` only if needed for coverage metadata; changed source/tests from Tasks 1–3.
Interfaces: Merge the approved delta into the current wiki after implementation; no new runtime interface.
Change: Run integrated checks, inspect native app when available, then update wiki evidence to match verified behavior.
Verify: `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings` pass; native sidebar inspection confirms create, switch, move, and remove or records the precise environment limit; wiki links and evidence resolve.
Progress: done
Result: `cargo fmt --check`, `cargo test --workspace` (574 Rune, 246 app-core, all daemon/protocol suites passed), `cargo build --workspace`, `cargo clippy --workspace -- -D warnings` pass; wiki delta merged into `docs/llm/domains/rune-chrome.md` (validator clean for changed pages; pre-existing `apps/desktop` source errors in unrelated pages remain). Native window inspection not run in this environment (no interactive display session); the sidebar behavior is covered by headless runtime tests.

## Files inspected, no change

- `apps/rune/src/data.rs` — checkout key and Unassigned session partitioning remain the source for grouping sessions.
- `apps/rune/src/editor/navigation.rs` — project/worktree selection and draft guard remain unchanged.
- `apps/rune/src/panes.rs` — terminal tabs and session attachment must remain unchanged by group switching.
- `docs/llm/domains/rune-editor.md` — current editor selection contract remains global and unchanged.

## Approval Gate

Status: approved

2026-11-11 — User approved joint design and tasks ("làm task") after design.md, tasks.md, and delta.md were presented; authorizes implementation through review.

## Decision Log

2026-11-11 — User chose single workspace membership, sidebar-only filtering with tabs/sessions retained, deleting a workspace transfers projects to Default, and Unassigned appears only in Default. Pending joint design and task approval.

# Rune UX improvements tasks

## Design

[Rune UX improvements](design.md)

## Global Constraints

No daemon, protocol, `goko.db` schema, or theme change. Remove worktree and Remove project always confirm; confirming Remove worktree keeps today's forced removal and never stops a session. A sidebar confirmation is never open while the draft guard is open. `⌘W` is not bound (it is window close). Shortcuts do not fire while the sidebar prompt, a confirmation, the draft guard, quick-open, or an IME composition owns input. New tab keeps starting a shell. The disconnected pill has no Retry button. Prune runs `git worktree prune` only and touches no folder or session. This change edits no requirement owned by `multi-project-workspaces`; its sidebar tasks (1, 2, 4, 5, 8) start after that change's Task 3 is done or merged.

## Impact map

Entry: `apps/rune/src/main.rs::view`, `on_key`, `State`; `apps/rune/src/sidebar.rs::sidebar`, `project_node`, `session_row`, `worktree_node`, `menu_layer`; `apps/rune/src/chrome.rs::header`, `tab_bar`, `controls_bar`; `apps/rune/src/panes.rs::terminal_workspace`; `apps/rune/src/editor/workspace.rs::workspace`, `empty_state`, `notice_bar`.
Flow: user press or shortcut → `State` mutation or `sessions.rs::launch_session`/`remove_worktree`/`remove_project` → editor backend project lane (`editor/backend.rs::Req`) or daemon call → reply application (`editor/backend/apply.rs`, `main.rs::apply_reply`) → re-render; sidebar selection → `editor/navigation.rs` draft guard.
State changes: in-memory `State` (new confirmation field, notice timestamp), `EditorState`; no SQLite change; Git repository metadata via `git worktree prune` only.
External effects: `git worktree remove --force` (unchanged), `git worktree prune` (new), daemon `session.create` (unchanged); no new files or network.
Change candidates: `apps/rune/src/sidebar.rs`, `chrome.rs`, `panes.rs`, `main.rs`, `sessions.rs`, `theme.rs`, `editor/workspace.rs`, `editor/backend.rs`, `editor/backend/apply.rs`, `crates/app-core/src/worktrees.rs`, their in-file tests, `docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`.
Verification: focused Rune view tests, `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, accessibility-tree and click checks against a throwaway-`GOKO_HOME` build.
Verified at: 93f5cc1bc5028bc7f0e6e17ca8cfd3659acbecdd

## Tasks

### Task 1 — Name every sidebar control

Files: `apps/rune/src/sidebar.rs::chevron`, `action_button`, `checkout_actions`, `session_actions`, `project_node`, `worktree_group`, `worktree_node`, `other_sessions_group`, `unassigned_node`; sidebar view tests.
Interfaces: Produces accessible names on all sidebar buttons; later tasks reuse the target-in-name pattern.
Change: Chevron/disclosure buttons get "Collapse <name>" / "Expand <name>" by state; row actions get "New session in <row name>", "More actions for <row name>", "Stop session <title>", "Close pane <title>". Visible text and `native_id` keys are unchanged.
Verify: Add a view test that renders a sidebar with two projects, one worktree group, and one session, then asserts no button element has an empty name and no two action buttons share a name. Before the change the test fails (unnamed chevrons); after it passes.
Progress: todo
Result: none

### Task 2 — Tooltips for dots, badge, truncation, and chips

Files: `apps/rune/src/sidebar.rs::checkout_status_of`, `session_row`, `project_node`, `worktree_node`; `apps/rune/src/theme.rs::missing_badge`, `usage_chip`; `apps/rune/src/chrome.rs::usage_chip_for`; tests.
Interfaces: Consumes Task 1 names; produces `.help` text only.
Change: Warning dot "Uncommitted changes"; muted dot "Git status unknown"; `missing` badge "This folder no longer exists on disk"; truncated project/worktree/session labels carry their full text as help; usage chips' help says the percentage is the share of that window already used.
Verify: View tests assert each element's help text for dirty true / null, a missing worktree, a label longer than its cell, and a chip with a limit.
Progress: todo
Result: none

### Task 3 — Notices close themselves

Files: `apps/rune/src/main.rs::State.notice`, `poll`, the toast block in `view`; `apps/rune/src/editor/workspace.rs::notice_bar`, `apps/rune/src/editor/state.rs::Notice`; tests.
Interfaces: Produces `notice_set_at` on `State`; the Editor notice bar gains a dismiss button clearing `EditorState.notice`.
Change: A notice set on `State` clears 8 seconds later from the existing poll tick; setting a new notice restarts the timer; the close button still clears at once. The Editor notice bar has a dismiss button.
Verify: Test with an injected clock: set a notice, advance 7 s → still shown; advance past 8 s → cleared; set a second notice at 7 s → survives until 15 s. Test that the Editor dismiss button clears `EditorState.notice`.
Progress: todo
Result: none

### Task 4 — Confirm Remove worktree and Remove project

Files: `apps/rune/src/main.rs::State` (confirmation field), `apps/rune/src/sidebar.rs::menu_layer`, new confirmation layer in the same file; `apps/rune/src/sessions.rs::remove_worktree`, `remove_project`; tests.
Interfaces: Produces `State.sidebar_confirm: Option<SidebarConfirm>` carrying the target, shown as a modal layer using the existing dialog kit; the menu items now set it instead of calling the removal.
Change: Remove worktree opens a modal naming the folder, saying whether it has uncommitted changes (row `dirty`) and the number of sessions running in it; Confirm calls the existing removal, Cancel/Escape clear it. Remove project opens a modal stating that only the registration is removed, its sessions become Unassigned, and files stay. A confirmation is not opened while `editor.guard` is open. If the target is gone from the lists at confirm time, show the notice "That item no longer exists" and remove nothing.
Verify: Tests: choosing Remove worktree enqueues nothing until Confirm; Cancel and Escape enqueue nothing; Confirm enqueues `Req::RemoveWorktree` once; dirty true shows the uncommitted-changes line; two attached sessions show "2 sessions"; a vanished target shows the notice and enqueues nothing; with the guard open the menu item does not open the modal. Same cases for Remove project.
Progress: todo
Result: none

### Task 5 — Start buttons, New tab menu, unavailable-agent reason

Files: `apps/rune/src/panes.rs::terminal_workspace`; `apps/rune/src/chrome.rs::tab_bar`; `apps/rune/src/sidebar.rs::menu_layer` (agent items); `apps/rune/src/sessions.rs::request_session_argv`, `launch_session`; `crates/app-core/src/agents.rs::Agent`; tests.
Interfaces: Consumes the agent list in `Data.agents` and `request_session_argv`; produces one shared function building agent menu entries with disabled reasons, used by the sidebar menu and the New tab menu.
Change: Empty Terminal view shows one button per available agent plus Shell for the active checkout, else the first sidebar project's root; with no projects, an Add Project button; buttons are disabled with a reason while disconnected. New tab keeps starting a shell and gains a chevron menu with every agent preset and Custom command…. An unavailable agent stays disabled and its entry says "`<binary>` not found on PATH". The Custom command… prompt is the existing sidebar input.
Verify: View tests: no tabs + two projects → buttons for available agents + Shell; no projects → Add Project only; disconnected → buttons disabled with reason; an agent with `available == false` renders a disabled entry carrying the PATH reason in both menus; pressing New tab still creates one shell; choosing an agent entry calls `request_session_argv` with the active checkout.
Progress: todo
Result: none

### Task 6 — App shortcuts

Files: `apps/rune/src/main.rs::on_key`; `apps/rune/src/panes.rs` (tab activation helper); `apps/rune/src/chrome.rs::settings_button`, `tab_bar` (help text with the shortcut); tests.
Interfaces: Consumes the draft-guarded `navigation::navigate(PendingNav::SetView)` and the New tab launch path from Task 5; produces the shortcut handler placed before the Terminal key forwarding.
Change: `⌘T` new shell tab; `⌘1`–`⌘9` activate tab N in the Terminal view; `⌘,` toggles Settings through the draft guard. Ignored while the sidebar prompt, a confirmation, the draft guard, quick-open, or IME preedit owns input. `⌘W` unchanged. Help text on New tab and Settings names the shortcut.
Verify: Tests with synthetic key events: `⌘T` launches one shell; `⌘2` activates the second tab, `⌘9` with two tabs does nothing; `⌘,` opens then closes Settings, and with a dirty draft raises the guard instead; with `sidebar_confirm` open or preedit non-empty none fire; `⌘T` is not forwarded to the terminal as bytes.
Progress: todo
Result: none

### Task 7 — Say why the daemon is disconnected

Files: `apps/rune/src/chrome.rs::header` (the Disconnected pill), `connection_dot`; `apps/rune/src/main.rs::State.daemon_reason`; tests.
Interfaces: Consumes the existing `daemon_reason`; produces no new state.
Change: While disconnected the pill reads "Reconnecting…" and its help text is the last reason; when the reason is empty the help says "Waiting for the Goko daemon". Connected rendering is unchanged.
Verify: View tests: connected → no pill; disconnected with a reason → pill text "Reconnecting…" and help equals the reason; disconnected with none → fallback help text.
Progress: todo
Result: none

### Task 8 — Prune missing worktrees

Files: `crates/app-core/src/worktrees.rs` (new `prune`); `apps/rune/src/editor/backend.rs::Req`, `run_job`; `apps/rune/src/editor/backend/apply.rs`; `apps/rune/src/sidebar.rs::menu_layer`; tests.
Interfaces: Produces `worktrees::prune(root) -> Result<(), BridgeError>` and `Req::PruneWorktrees { project_id }`, which refreshes that project's worktree list on success.
Change: The More menu of a git project row lists **Prune missing worktrees** only when at least one of its worktrees is `missing`; it runs `git worktree prune` in the project root and refreshes the list; failure shows the error notice and leaves the list unchanged.
Verify: app-core test: temp repo with a linked worktree whose folder is deleted → `prune` removes its record and keeps a live linked worktree; a non-repo path returns an error. Rune test: item absent with no missing worktree, present with one; choosing it enqueues `Req::PruneWorktrees` once; error reply shows a notice.
Progress: todo
Result: none

### Task 9 — Editor empty state lists projects

Files: `apps/rune/src/editor/workspace.rs::empty_state`; `apps/rune/src/editor/navigation.rs::PendingNav::SelectCheckout` (consumed); tests.
Interfaces: Consumes `EditorState.projects`; selecting a row goes through the draft-guarded `navigate`.
Change: With no checkout, the empty Editor lists up to five registered projects (name and root); pressing one selects its root checkout; **Add project** remains. Loading and error text keep their current wording.
Verify: View tests: three projects → three rows and Add project; seven projects → five rows; pressing a row selects that project's root checkout through the guard; zero projects → today's text and Add project; loading → "Loading projects…".
Progress: todo
Result: none

### Task 10 — Integrate, verify in the app, document

Files: `docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`; all files changed by Tasks 1–9.
Interfaces: Merges the approved delta; no runtime interface.
Change: Run integrated checks; build and launch against a throwaway `GOKO_HOME` (never the user's running instance) and check with the accessibility tree and clicks: no unnamed sidebar button, Remove worktree/project confirm, `⌘T`/`⌘1`/`⌘,` work, the empty Terminal and Editor states render; then merge the delta into the wiki with evidence paths.
Verify: `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings` pass; the native checks above pass or the environment limit is recorded in this result; wiki links and evidence resolve.
Progress: todo
Result: none

## Files inspected, no change

- `apps/rune/src/daemon.rs` — the supervisor already redials with capped backoff and reports `Disconnected { reason }`, so no Retry button.
- `apps/rune/src/editor/state.rs` — confirmed the editor holds one `doc`, so file tabs are out of scope.
- `crates/app-core/src/git_write.rs` — confirmed no pull/fetch/branch operations exist, so they are out of scope.
- `apps/rune/src/settings.rs` — its inline account-remove confirmation is the in-repo precedent for confirm wording.

## Approval Gate

Status: pending

## Decision Log

None.

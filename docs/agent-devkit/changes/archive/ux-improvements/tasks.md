# Rune UX improvements tasks

## Design

[Rune UX improvements](design.md)

## Global Constraints

No daemon, protocol, `goko.db` schema, or theme change. Remove worktree and Remove project always confirm, in an inline row like the workspace removal confirmation; confirming Remove worktree keeps today's forced removal and never stops a session. Only one sidebar confirmation is open at a time, including the workspace removal row. `⌘W` is not bound (it is window close). Shortcuts do not fire while the sidebar prompt, a confirmation, the draft guard, quick-open, or an IME composition owns input. New tab keeps starting a shell. The disconnected pill has no Retry button. Prune runs `git worktree prune` only and touches no folder or session. `multi-project-workspaces` is archived and its sidebar code is the base; this change edits none of its requirement IDs.

## Impact map

Entry: `apps/rune/src/main.rs::view`, `on_key`, `State`; `apps/rune/src/sidebar.rs::sidebar`, `workspace_header`, `workspace_remove_confirm_row`, `small_button`, `project_node`, `session_row`, `worktree_node`, `menu_layer`; `apps/rune/src/chrome.rs::header`, `tab_bar`, `controls_bar`; `apps/rune/src/panes.rs::terminal_workspace`; `apps/rune/src/editor/workspace.rs::workspace`, `empty_state`, `notice_bar`.
Flow: user press or shortcut → `State` mutation or `sessions.rs::launch_session`/`remove_worktree`/`remove_project` → editor backend project lane (`editor/backend.rs::Req`) or daemon call → reply application (`editor/backend/apply.rs`, `main.rs::apply_reply`) → re-render; sidebar selection → `editor/navigation.rs` draft guard.
State changes: in-memory `State` (new confirmation field, notice timestamp), `EditorState`; no SQLite change; Git repository metadata via `git worktree prune` only.
External effects: `git worktree remove --force` (unchanged), `git worktree prune` (new), daemon `session.create` (unchanged); no new files or network.
Change candidates: `apps/rune/src/sidebar.rs`, `chrome.rs`, `panes.rs`, `main.rs`, `sessions.rs`, `theme.rs`, `editor/workspace.rs`, `editor/backend.rs`, `editor/backend/apply.rs`, `crates/app-core/src/worktrees.rs`, their in-file tests, `docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`.
Verification: focused Rune view tests, `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, accessibility-tree and click checks against a throwaway-`GOKO_HOME` build.
Verified at: 93f5cc1bc5028bc7f0e6e17ca8cfd3659acbecdd plus the uncommitted `multi-project-workspaces` working tree (the sidebar baseline for this change)

## Tasks

### Task 1 — Name every sidebar control

Files: `apps/rune/src/sidebar.rs::chevron`, `action_button`, `checkout_actions`, `session_actions`, `project_node`, `worktree_group`, `worktree_node`, `other_sessions_group`, `unassigned_node`; sidebar view tests.
Interfaces: Produces accessible names on all sidebar buttons; later tasks reuse the target-in-name pattern.
Change: Chevron/disclosure buttons get "Collapse <name>" / "Expand <name>" by state; row actions get "New session in <row name>", "More actions for <row name>", "Stop session <title>", "Close pane <title>". Visible text and `native_id` keys are unchanged.
Verify: Add a view test that renders a sidebar with two projects, one worktree group, and one session, then asserts no button element has an empty name and no two action buttons share a name. Before the change the test fails (unnamed chevrons); after it passes.
Progress: done
Result: `cargo test -p goko-rune every_sidebar_button_names_its_target` — failed before the change (unnamed add-project row), passes after

### Task 2 — Tooltips for dots, badge, truncation, and chips

Files: `apps/rune/src/sidebar.rs::checkout_status_of`, `session_row`, `project_node`, `worktree_node`; `apps/rune/src/theme.rs::missing_badge`, `usage_chip`; `apps/rune/src/chrome.rs::usage_chip_for`; tests.
Interfaces: Consumes Task 1 names; produces `.help` text only.
Change: Warning dot "Uncommitted changes"; muted dot "Git status unknown"; `missing` badge "This folder no longer exists on disk"; truncated project/worktree/session labels carry their full text as help; usage chips' help says the percentage is the share of that window already used.
Verify: View tests assert each element's help text for dirty true / null, a missing worktree, a label longer than its cell, and a chip with a limit.
Progress: done
Result: `cargo test -p goko-rune -- status_marks_and_truncated a_usage_chip_says` — both failed before the change, both pass after

### Task 3 — Notices close themselves

Files: `apps/rune/src/main.rs::State.notice`, `poll`, the toast block in `view`; `apps/rune/src/editor/workspace.rs::notice_bar`, `apps/rune/src/editor/state.rs::Notice`; tests.
Interfaces: Produces `notice_set_at` on `State`; the Editor notice bar gains a dismiss button clearing `EditorState.notice`.
Change: A notice set on `State` clears 8 seconds later from the existing poll tick; setting a new notice restarts the timer; the close button still clears at once. The Editor notice bar has a dismiss button.
Verify: Test with an injected clock: set a notice, advance 7 s → still shown; advance past 8 s → cleared; set a second notice at 7 s → survives until 15 s. Test that the Editor dismiss button clears `EditorState.notice`.
Progress: done
Result: `cargo test -p goko-rune -- notice` — 11 passed, 0 failed (includes the 2 expiry tests and the Editor dismiss test). Known limit: a notice is detected by text change, so re-setting identical text while the toast is visible does not restart its timer

### Task 4 — Confirm Remove worktree and Remove project

Files: `apps/rune/src/main.rs::State` (confirmation field next to `workspace_remove_confirm`), `apps/rune/src/sidebar.rs::menu_layer`, `workspace_remove_confirm_row`, `small_button`; `apps/rune/src/sessions.rs::remove_worktree`, `remove_project`; tests.
Interfaces: Produces `State.sidebar_confirm: Option<SidebarConfirm>` carrying the target, shown as an inline row built from `small_button` like `workspace_remove_confirm_row`; the menu items now set it instead of calling the removal, and setting either confirmation clears the other.
Change: Remove worktree opens an inline row naming the folder, saying whether it has uncommitted changes (row `dirty`) and the number of sessions running in it; Confirm calls the existing removal, Cancel clears it. Remove project opens an inline row stating that only the registration is removed, its sessions become Unassigned, and files stay. If the target is gone from the lists at confirm time, show the notice "That item no longer exists" and remove nothing.
Verify: Tests: choosing Remove worktree enqueues nothing until Confirm; Cancel enqueues nothing; Confirm enqueues `Req::RemoveWorktree` once; dirty true shows the uncommitted-changes line; two attached sessions show "2 sessions"; a vanished target shows the notice and enqueues nothing; opening one confirmation clears the other. Same cases for Remove project.
Progress: done
Result: `cargo test -p goko-rune -- remove_worktree_waits cancel_on_a dirty_worktree clean_or_unknown two_attached vanished_ opening_a_removal remove_project_waits` — 10 failed before the change, all pass after; full `cargo test -p goko-rune` 599 passed (589 + 10 new); `cargo clippy --workspace -- -D warnings` clean

### Task 5 — Start buttons, New tab menu, unavailable-agent reason

Files: `apps/rune/src/panes.rs::terminal_workspace`; `apps/rune/src/chrome.rs::tab_bar`; `apps/rune/src/sidebar.rs::menu_layer` (agent items); `apps/rune/src/sessions.rs::request_session_argv`, `launch_session`; `crates/app-core/src/agents.rs::Agent`; tests.
Interfaces: Consumes the agent list in `Data.agents` and `request_session_argv`; produces one shared function building agent menu entries with disabled reasons, used by the sidebar menu and the New tab menu.
Change: Empty Terminal view shows one button per available agent plus Shell for the active checkout, else the first project visible in the selected workspace; with no project in it, an Add Project button; buttons are disabled with a reason while disconnected. New tab keeps starting a shell and gains a chevron menu with every agent preset and Custom command…. An unavailable agent stays disabled and its entry says "`<binary>` not found on PATH". The Custom command… prompt is the existing sidebar input.
Verify: View tests: no tabs + two projects → buttons for available agents + Shell; selected workspace empty → Add Project only; disconnected → buttons disabled with reason; an agent with `available == false` renders a disabled entry carrying the PATH reason in both menus; pressing New tab still creates one shell; choosing an agent entry calls `request_session_argv` with the active checkout.
Progress: done
Result: `cargo test -p goko-rune -- an_empty_terminal a_workspace_without disconnected_start an_unavailable_agent new_tab_still choosing_an_agent` — 5 failed before the change and `new_tab_still_starts_one_shell` passed before (it guards existing behaviour); all 6 pass after; full `cargo test -p goko-rune` 605 passed (599 + 6); `cargo clippy --workspace -- -D warnings` clean

### Task 6 — App shortcuts

Files: `apps/rune/src/main.rs::on_key`; `apps/rune/src/panes.rs` (tab activation helper); `apps/rune/src/chrome.rs::settings_button`, `tab_bar` (help text with the shortcut); tests.
Interfaces: Consumes the draft-guarded `navigation::navigate(PendingNav::SetView)` and the New tab launch path from Task 5; produces the shortcut handler placed before the Terminal key forwarding.
Change: `⌘T` new shell tab; `⌘1`–`⌘9` activate tab N in the Terminal view; `⌘,` toggles Settings through the draft guard. Ignored while the sidebar prompt, a confirmation, the draft guard, quick-open, or IME preedit owns input. `⌘W` unchanged. Help text on New tab and Settings names the shortcut.
Verify: Tests with synthetic key events: `⌘T` launches one shell; `⌘2` activates the second tab, `⌘9` with two tabs does nothing; `⌘,` opens then closes Settings, and with a dirty draft raises the guard instead; with `sidebar_confirm` open or preedit non-empty none fire; `⌘T` is not forwarded to the terminal as bytes.
Progress: done
Result: `cargo test -p goko-rune -- cmd_t_ cmd_digit cmd_comma shortcuts_do_not` — 7 passed, 0 failed (the six that fire a shortcut failed before the change; `shortcuts_do_not_fire_under_a_confirmation_or_a_composition` passed before too, as a negative guard); full `cargo test -p goko-rune` 612 passed (605 + 7); `cargo clippy --workspace -- -D warnings` clean

### Task 7 — Say why the daemon is disconnected

Files: `apps/rune/src/chrome.rs::header` (the Disconnected pill), `connection_dot`; `apps/rune/src/main.rs::State.daemon_reason`; tests.
Interfaces: Consumes the existing `daemon_reason`; produces no new state.
Change: While disconnected the pill reads "Reconnecting…" and its help text is the last reason; when the reason is empty the help says "Waiting for the Goko daemon". Connected rendering is unchanged.
Verify: View tests: connected → no pill; disconnected with a reason → pill text "Reconnecting…" and help equals the reason; disconnected with none → fallback help text.
Progress: done
Result: `cargo test -p goko-rune -- chrome connected_header disconnected_pill` — 5 passed, 0 failed (the two disconnected tests failed before the change)

### Task 8 — Prune missing worktrees

Files: `crates/app-core/src/worktrees.rs` (new `prune`); `apps/rune/src/editor/backend.rs::Req`, `run_job`; `apps/rune/src/editor/backend/apply.rs`; `apps/rune/src/sidebar.rs::menu_layer`; tests.
Interfaces: Produces `worktrees::prune(root) -> Result<(), BridgeError>` and `Req::PruneWorktrees { project_id }`, which refreshes that project's worktree list on success.
Change: The More menu of a git project row lists **Prune missing worktrees** only when at least one of its worktrees is `missing`; it runs `git worktree prune` in the project root and refreshes the list; failure shows the error notice and leaves the list unchanged.
Verify: app-core test: temp repo with a linked worktree whose folder is deleted → `prune` removes its record and keeps a live linked worktree; a non-repo path returns an error. Rune test: item absent with no missing worktree, present with one; choosing it enqueues `Req::PruneWorktrees` once; error reply shows a notice.
Progress: done
Result: `cargo test -p app-core prune` — 2 passed; `cargo test -p goko-rune prune` — 6 passed, 0 failed (4 of the 5 new Rune tests failed before the change; the item-absent test passed before as a guard; the app-core tests were written alongside `prune` and not seen failing first)

### Task 9 — Editor empty state lists projects

Files: `apps/rune/src/editor/workspace.rs::empty_state`; `apps/rune/src/editor/navigation.rs::PendingNav::SelectCheckout` (consumed); tests.
Interfaces: Consumes `EditorState.projects`; selecting a row goes through the draft-guarded `navigate`.
Change: With no checkout, the empty Editor lists up to five registered projects (name and root); pressing one selects its root checkout; **Add project** remains. Loading and error text keep their current wording.
Verify: View tests: three projects → three rows and Add project; seven projects → five rows; pressing a row selects that project's root checkout through the guard; zero projects → today's text and Add project; loading → "Loading projects…".
Progress: done
Result: `cargo test -p goko-rune -- three_projects seven_projects pressing_a_row zero_projects_keep loading_projects_says` — 5 passed, 0 failed (3 failed before the change; zero-projects and loading passed before as guards); full `cargo test --workspace` 992 passed, 0 failed (goko-rune 622 = 612 + 10 new); `cargo clippy --workspace -- -D warnings` clean

### Task 10 — Show attention hidden by the workspace filter

Files: `apps/rune/src/sidebar.rs::workspace_header`, workspace selector menu in `menu_layer`; `apps/rune/src/data.rs::partition`; `apps/rune/src/sessions.rs::session_shown_activity`; tests.
Interfaces: Consumes `ProjectView.project.workspace_id`, the session-to-project mapping from `data::partition`, and `session_shown_activity`; produces a per-workspace count of sessions whose activity is `Waiting`.
Change: The selector shows a warning dot when any workspace other than the selected one has a waiting session; the selector menu appends "· N need you" to each such workspace's entry. Counts use the same activity value the session row's dot uses, so the two never disagree. The dot's help text reads "Another workspace needs you".
Verify: View tests: two workspaces, a waiting session in the hidden one → dot present and menu entry reads "· 1 need you"; the session becomes `Done` → dot and suffix disappear; waiting session in the selected workspace → no dot; a project moved to the other workspace moves its count; Unassigned waiting session counts for Default only.
Progress: done
Result: `cargo test -p goko-rune -- the_selector_flags the_flag_clears a_waiting_session_in_the_selected a_moved_project_moves an_unassigned_waiting` — 4 of 5 failed before the change (the selected-workspace case passes before as a guard), all 5 pass after; full `cargo test --workspace` passes (goko-rune 627 = 622 + 5), `cargo fmt --all -- --check` and `cargo clippy --workspace -- -D warnings` clean

### Task 11 — Integrate, verify in the app, document

Files: `docs/llm/domains/rune-chrome.md`, `docs/llm/domains/rune-editor.md`; all files changed by Tasks 1–10.
Interfaces: Merges the approved delta; no runtime interface.
Change: Run integrated checks (Tasks 1–10); build and launch against a throwaway `GOKO_HOME` (never the user's running instance) and check with the accessibility tree and clicks: no unnamed sidebar button, Remove worktree/project confirm, `⌘T`/`⌘1`/`⌘,` work, the empty Terminal and Editor states render, the workspace selector flags a waiting session in another workspace; then merge the delta into the wiki with evidence paths.
Verify: `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings` pass; the native checks above pass or the environment limit is recorded in this result; wiki links and evidence resolve.
Progress: done
Result: `cargo fmt --check`, `cargo build --workspace`, `cargo test --workspace` (627 Rune, 249 app-core, all daemon/protocol suites, 0 failed), `cargo clippy --workspace -- -D warnings` pass. Real app (separate `GokoQA` bundle, own `GOKO_HOME`, the user's running instance untouched): accessibility tree shows 0 unnamed buttons (16 before); the empty Terminal view offers Claude Code, Codex, OpenCode, Devin, Antigravity and Shell; Remove worktree opens the inline confirmation ("Remove alpha-gone? Git status unknown. No sessions run in it.") and removes nothing until Remove; Prune missing worktrees removes only the deleted worktree's Git record and its menu item then disappears; `⌘T` started a Shell tab. Not verified in the real app: `⌘,`, `⌘1`–`⌘9`, the workspace attention dot, and the New tab menu entries (the automation tool could not deliver keys to the window after the first press); they are covered by view tests. The check also found the first-launch migration race fixed in Task 13. Wiki: delta merged into `docs/llm/domains/rune-chrome.md` (10 requirements, `RNC-header-status` modified) and `rune-editor.md` (2 requirements); the validator reports no error on either page (one warning: `RNC-new-tab-menu` vs `RNC-new-tab` slugs are near-duplicates but describe different behavior); its remaining errors are in older pages that still cite the removed `apps/desktop`.

### Task 12 — Harden workspace mutations (run first)

Files: `apps/rune/src/main.rs::State::submit_workspace_create`, `submit_workspace_rename`, `select_workspace`, `remove_workspace`, `move_project`; tests.
Interfaces: None new; no dependency on other tasks. Run before Task 1 because it edits the same `State` methods the later tasks read.
Change: A name submission is ignored while another workspace write is in flight (`workspace_pending` is set), so a double Enter sends one request. When the editor queue refuses `SetSetting`, `RemoveWorkspace`, or `MoveProject`, the user gets the notice "the editor queue is full; try again" instead of a silent no-op, and a refused selection write is not reported as saved. No schema change: a `UNIQUE` index on workspace names is left out because a migration would fail on a database that already holds duplicates.
Verify: Tests: submitting twice with a reply still pending enqueues one `CreateWorkspace`; the same for rename; with the queue full, selecting, removing, and moving each set the notice and leave state unchanged; after the first reply the next submission is accepted. These fail before the change and pass after.
Progress: done
Result: `cargo test -p goko-rune -- workspace a_second_ a_refused_ the_next_submission` — 27 passed, 0 failed (5 new tests failed before the change; the accepted-after-reply test guards the release and passed before). Review fix: a refused selection write now still applies the selection and says it was not saved, because `RemoveWorkspace` and `CreateWorkspace` replies call `select_workspace` and must not leave a deleted workspace selected; full `cargo test -p goko-rune` 586 passed, `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` clean

### Task 13 — Make the first-launch migration race-safe (found while verifying in the app)

Files: `crates/app-core/src/store.rs::migrate`, `store` tests.
Interfaces: None; the schema and `MIGRATIONS` are unchanged.
Change: The app opens `goko.db` from more than one thread at startup (`editor/backend.rs` and `editor/acp.rs` both call `Storage::open`). `migrate` read `user_version` outside its transaction, so on a fresh home two openers both ran the first migration and the loser failed with "cannot migrate goko.db: table projects already exists", which the sidebar showed as two red lines. Each migration step now begins an IMMEDIATE transaction and re-reads `user_version` inside it, skipping a step another opener already applied.
Verify: A test opens one fresh home from four threads, 20 times, and requires every open to succeed; it failed before with the error above and passes after; the existing upgrade and reopen tests still pass; a rollback test and a test that an up-to-date store opens while another connection holds the write lock were added in the review fixes.
Progress: done
Result: `cargo test -p app-core` — 251 passed (new `concurrent_first_opens_all_succeed` failed before, passed 3 of 3 reruns after); in the real app, a fresh `GOKO_HOME` now opens with no migration error and `user_version` 2

## Files inspected, no change

- `apps/rune/src/daemon.rs` — the supervisor already redials with capped backoff and reports `Disconnected { reason }`, so no Retry button.
- `apps/rune/src/editor/state.rs` — confirmed the editor holds one `doc`, so file tabs are out of scope.
- `crates/app-core/src/git_write.rs` — confirmed no pull/fetch/branch operations exist, so they are out of scope.
- `apps/rune/src/settings.rs` — its inline account-remove confirmation is the in-repo precedent for confirm wording.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-11 — The user approved the plan and asked to also fix the review findings: Task 12 added (double-submit guard and queue-full notices; no schema change). Reviewer findings on `RemoveWorkspace` re-listing, restore after a failed list, and a name `UNIQUE` index are not fixed here (negligible probability or migration risk). Approval covers implementation through review.
- 2026-11-11 — Task 13 added during verification: a first-launch migration race in `store.rs` that showed two red errors in the sidebar on a fresh home. It is a bug fix with no schema or behavior change, found by the real-app check this plan requires.
- 2026-11-11 — Independent review of the finished change found five real defects, fixed in place (no scope, behavior-contract, schema or interface change): failed Remove worktree/Remove project were silent (now a notice, and a refused enqueue also says so); repeated sidebar names shared accessible names (now numbered); `⌘` shortcuts ignored open editor pickers, the sidebar menu, and Shift/Option chords, and a stale composition blocked them outside the Terminal (now gated); the notice toast closed late on an idle app (now wakes at its deadline); the Remove worktree confirmation could show a stale `dirty` before a forced delete (now refreshed when opened, read live, and ended sessions are not counted). Migration now skips the write lock for an up-to-date database. Auto-repeat of `⌘T` is not filtered because the key event has no repeat flag. Known follow-ups, out of scope: the workspace tab strip has repeated accessible names ("zsh", "Close zsh"); `Prune missing worktrees` affects every missing worktree of the project and its menu entry does not say so. Fresh checks after the fixes: `cargo test --workspace` 639 Rune and 251 app-core passed, `fmt` and `clippy` clean.

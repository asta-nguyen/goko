# Project Map tasks

## Design

[Project Map](design.md)

## Global Constraints

- No database, daemon, protocol, dependency, workspace-membership, or project-file changes. No graph canvas, pan/zoom, drag-and-drop, recent-project history, worktree/session nodes, or terminal-session activation.
- Treat project selection as one guarded navigation intent. Workspace selection, checkout identity, highlight, and reveal happen only after the existing Save/Discard/Cancel guard permits the switch.
- Sidebar and editor project-picker selection share the approved project-terminal filtering behavior.
- Selections are ephemeral except for the existing selected-workspace setting. Do not persist the map query, keyboard cursor, or project history.

## Impact map

Entry: project_map::open in apps/rune/src/project_map.rs, called by main::app_shortcut (⌘K) and sidebar::sidebar (Project Map button). main::on_key and project_map::input_key route modal input.
Flow: project_map::layers/rows → activate → workspace-only State::select_workspace, or navigation::navigate(SelectMappedProject) → draft guard → apply re-resolves current project/owner → select_workspace + select_checkout → Tx::scroll_into_view_in against the new keyed sidebar layout. main::poll reconciles the cursor after data replies.
State changes: project_map open/query/cursor/return-focus are memory-only; selected_workspace is remembered with existing rune.selected_workspace setting; checkout changes reset per-checkout caches only after guard approval. No extra reveal state is needed because Rune applies Tx reveal requests to the committed layout (verified by distant-row runtime test and native QA).
External effects: existing worker handles settings writes and checkout/worktree reads; only the guard's explicit Save writes a document. No map-driven daemon/session, membership, schema or project-file mutation.
Change candidates: apps/rune/src/project_map.rs, apps/rune/src/main.rs, apps/rune/src/sidebar.rs, apps/rune/src/editor/navigation.rs, docs/llm/domains/rune-chrome.md. These are the implemented/inspected paths.
Verification: cargo fmt --check, cargo test --workspace (1031 passed, 22 suites, including 19 Project Map tests), cargo build --workspace and cargo clippy --workspace -- -D warnings passed. Native QA covered dark/light rendering, filtering/paste, cross-workspace selection/reveal, workspace-only preservation and draft Cancel/Discard. Wiki validation remains blocked as recorded below.
Verified at: 884e744906485f67c90512597a3abc41bc0699da

## Tasks

### Task 1 — Guard a project jump across workspaces

Files: apps/rune/src/editor/navigation.rs::PendingNav, navigate, apply and tests; apps/rune/src/main.rs::State and tests.
Interfaces: Adds internal SelectMappedProject { project_id }; consumes registered project/workspace lists, existing guard, State::select_workspace and select_checkout; exposes a post-commit sidebar reveal request for Task 3.
Change: Introduce one guarded intent that re-resolves target project/root/owner at application time before mutating state. After approval select the owning workspace and project root while preserving the view, stored tabs, pane layouts, and session processes; terminal visibility and focus follow the newly active project. Reject vanished IDs/owners without changing either selection. Preserve existing Save/Discard/Cancel and settings failure behavior. Workspace-only navigation continues through select_workspace.
Verify: cargo test -p goko-rune project_map — add cases for cross-workspace success, workspace-only preservation, Cancel and failed Save with no destination settings request, successful Save/Discard, moved/removed project while guard waits, removed owner, settings queue failure, startup restore and unchanged terminal state. Run cargo test -p goko-rune project_switch to retain existing guard behavior. Expected: all pass; no session operation is queued by a map jump.
Progress: done
Result: cargo test -p goko-rune project_map — 5 guard/navigation tests passed within the full workspace suite, including successful/failed save and settings refusal; baseline cargo test -p goko-rune — 639 passed.

### Task 2 — Render the searchable workspace/project map

Files: apps/rune/src/project_map.rs::ProjectMapState, rows, layers, key (new); apps/rune/src/main.rs::module declaration and State initialization.
Interfaces: Consumes current State workspace/project data and Task 1 guarded intent; produces controlled modal layers and open/close/input handling for Task 3. No new backend request types.
Change: Build a scrollable grouped hierarchy with clickable workspace headings and indented project buttons, text/icon current markers, project paths, search input and close control. Use existing Rune/rune-kit/theme primitives. Filter trimmed case-insensitive substrings of workspace/project/path; retain empty groups only for empty query. Distinguish loading, read failure with existing refresh retry, empty workspace/project list and no results. Rebuild rows on current data; retain cursor by ID or choose first row. Support bounded Up/Down, Enter, Tab, Escape, scrim dismissal and long-label help.
Verify: cargo test -p goko-rune project_map — add row/filter cases for Default, membership, duplicates, empty/whitespace query, workspace-name match, path match, no results, moved/removed rows, cursor clamping, loading/error selection disabled until initial lists are ready. Add Rune runtime cases for pointer versus Enter targets, close routes and current selection versus keyboard cursor. Expected: correct stable target IDs and no selection on dismissal.
Progress: done
Result: cargo test --workspace — 1031 tests passed across 22 suites, including grouping, filtering, stable identities and cursor/readiness cases.

### Task 3 — Integrate global input, sidebar entry and reveal

Files: apps/rune/src/main.rs::view, on_key, app_shortcut and paste routing; apps/rune/src/sidebar.rs::workspace_header, sidebar and project row keys; apps/rune/src/project_map.rs::open, close, key and runtime tests.
Interfaces: Mounts Task 2 layers; consumes Task 1 committed reveal request and existing Tx::scroll_into_view_in. Uses existing sidebar-project:<id> keys and a stable sidebar scroll-scope key.
Change: Add accessible Project Map button beside workspace selector and plain ⌘K in Terminal/Editor. Respect every existing input owner, Settings and terminal IME; map owns input/paste before terminal forwarding or editor shortcuts. Close before presenting a draft guard; restore previous usable focus on dismissal. After guarded commit render the destination workspace then reveal its keyed project row. Keep workspace-only clicks distinct from project clicks. Ensure visual and keyboard row focus stays visible at narrow sizes and in both themes.
Verify: cargo test -p goko-rune project_map and cargo test -p goko-rune shortcuts — cover button/chord opening, modified chord and modal/Settings/IME suppression, typing/paste and shortcut isolation, Enter/Escape and focus restoration, below-viewport sidebar reveal after cross-workspace commit, unchanged view/tabs/session and loading/error transitions. Native check through apps/rune/run-macos.sh debug for actual reveal, focus, narrow bounds and light/dark appearance. Expected: selections and text never leak to underlying surfaces; record a verification limit if native inspection cannot run.
Progress: done
Result: cargo test --workspace — 1031 passed, including 19 Project Map tests for guard lifecycle, input/paste/IME, focus/scrim, narrow light/dark bounds and distant reveal. Native macOS QA with isolated GOKO_HOME verified shortcut/button, search/paste, Work → Default project 31 reveal, workspace-only selection retaining Project 31, Editor draft Cancel preserving Work/Project 00, Discard switching to Personal/Project 02, and dark/light rendering. Independent read-only review found no remaining findings; its IME concern was disproved by pinned Rune routing and the runtime composition test.

### Task 4 — Document the verified behavior and run workspace checks

Files: docs/llm/domains/rune-chrome.md::Requirements and Sources; sibling delta.md::RNC-project-map for merge verification; sibling tasks.md::task results.
Interfaces: Consumes implemented behavior and evidence from Tasks 1–3; merges the approved delta through document-wiki Task updates, without adding links from the wiki to process artifacts.
Change: Record actual map entry, hierarchy/search, workspace/project click differences, draft guard atomicity, unchanged terminal session/view, modal input ownership and stale-data behavior. Check current source and tests for every added scenario; leave unrelated wiki gaps outside this task. Record native verification limits explicitly.
Verify: cargo fmt --check; cargo test --workspace; cargo build --workspace; cargo clippy --workspace -- -D warnings. Verify delta requirement matches the merged wiki block and all relative links/evidence paths exist. Expected: all four checks pass or concrete blockers remain reported; no claim of native visual verification without inspection.
Progress: todo
Result: Code checks pass (build/fmt/clippy and workspace tests); Project Map requirement and source-backed flow/state/error/test sections were merged into rune-chrome.md. Full wiki validator fails with 160 existing missing-path errors in unchanged legacy desktop pages; zero errors in the changed rune-chrome.md. See Verification limits below. This documentation gate remains open.

## Files inspected, no change

- apps/rune/src/editor/quick_open.rs — existing TextEdit/modal/input pattern and reserved ⌘P.
- apps/rune/src/editor/keyboard.rs — modal ownership and existing Tx::scroll_into_view_in usage at lines 490–510.
- apps/rune/src/editor/workspace.rs — project dropdown keeps its existing guarded SelectProject callback.
- apps/rune/src/editor/state.rs — existing project/checkout state and project-picker ownership.
- crates/app-core/src/store.rs — Project.workspace_id and existing persisted workspace membership; no schema change.
- docs/llm/domains/rune-editor.md — existing checkout guard contract.
- docs/agent-devkit/changes/archive/multi-project-workspaces/design.md — historical workspace-only behavior.
- docs/agent-devkit/changes/archive/ux-improvements/design.md — historical shortcut and attention behavior.

## Approval Gate

Status: approved

Approved 2026-11-11: user requested “ok implement đi” after reviewing design.md, tasks.md and delta.md. Implementation through final review is authorized.

## Decision Log

- 2026-11-11: User approved “implement đi” for the bounded project terminal filter follow-up: retain background processes/layouts, but display only active-project sessions/tabs and restore project focus. This supersedes the original map requirement to keep a foreign terminal selected. See [approved delta](../project-terminal-filter/delta.md).

- 2026-11-11: User accepted the Project Map direction and requested a plan. Keep the current Terminal/Editor view when navigating; show workspace membership and allow direct clicks. Written plan approval is pending.
- 2026-11-11: User approved the written files and requested implementation.

## Verification limits

- Full wiki validator: `node /Users/nus/projects/goko/.agents/skills/document-wiki/scripts/validate-llm-wiki.mjs /Users/nus/projects/goko` exits 1 with 160 missing-path errors: architecture/editor.md (86), architecture/overview.md (31), domains/desktop-settings.md (19), domains/desktop-theme.md (24). All four files are unchanged from HEAD and refer to the removed apps/desktop tree. The changed rune-chrome.md has zero errors, plus the existing near-duplicate slug warning for RNC-new-tab/RNC-new-tab-menu. Full diagnostics: target/project-map-qa/wiki-validation.log. Fixing those historical pages is outside the approved Project Map scope. The final review cannot pass or archive this change while the required whole-wiki validator fails.
- Native QA used a separate bundle and target/project-map-qa/home; no user project registration/settings/session data was modified. Both theme renderings, native filter/paste and cross-workspace navigation, distant sidebar reveal, Cancel/Discard and draft focus were observed. Minimum-window bounds were verified through Rune runtime tests rather than a native window resize.

## Final review

Status: fail — documentation gate only. Build, formatting, Clippy, all 1031 workspace tests, exact delta/wiki comparison, evidence paths, plan links and git diff --check pass. Independent three-file review plus orchestrator sidebar/caller/complexity review found no remaining source findings. The whole-wiki validator still reports 160 errors exclusively in unchanged legacy Desktop pages; no archive move or merge marker was applied. No commit/push was made.

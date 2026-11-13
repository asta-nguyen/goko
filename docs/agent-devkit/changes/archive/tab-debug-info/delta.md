# Delta

Approved: 2026-11-11 — right-click tab copy actions for Goko and agent IDs, debug information, and approximate local process-tree RAM for every supported agent.
Merged into docs/llm/: 2026-11-13

## Impact map

Entry: `apps/rune/src/chrome.rs` tab_el → secondary press on a tab.
Flow: tab menu → tab's focused pane → Goko session → worker process snapshot → clipboard via Tx::copy.
State changes: ephemeral menu and process samples; no persisted session or agent configuration changes.
External effects: read local process metadata and agent-owned session metadata; clipboard writes only on selection.
Change candidates: Rune tab menu, State, worker polling, app-core diagnostics, Rune chrome wiki.
Verification: cargo build --workspace; cargo fmt --check; cargo clippy --workspace -- -D warnings; cargo test --workspace; headless Rune menu and process/identity tests.
Verified at: 4ff367ebaabc549128fb297dc25f7f45a5e921f8

## Verification

- Baseline: 1,067 workspace tests passed; format and Clippy passed before source edits.
- Implementation: workspace build, format and Clippy passed; 1,076 workspace tests passed, including nine new diagnostics/menu tests.
- Independent read-only review checked the menu, process/identity logic and worker/input integration. Its redundant shortcut guard finding was removed.
- Wiki requirement and task behavior are merged into the Rune chrome page. Its validator diagnostics contain zero errors; baseline and current full-wiki validation both report the same 160 errors in legacy pages referencing the removed desktop app, with zero added errors.
- Final review remains open because the full wiki validator fails. No archive or merge-complete marker is claimed. Legacy wiki repair is outside this change's approved scope.
- Native-window manual testing and commits were not performed. Headless Rune tests exercise secondary press, clipboard contents, inactive-tab targeting and Escape dismissal.

## ADDED Requirements

### RNC-tab-debug

Page: `docs/llm/domains/rune-chrome.md`
Search: searched all RNC requirements and open deltas; no existing requirement covers tab debug copy actions or RAM.

Requirement:

````md
### RNC-tab-debug

The system SHALL offer a secondary-press tab menu with Copy Tab ID, Copy Goko Session ID, Copy Agent Session ID, and Copy Debug Info, resolving session actions against the target tab's focused pane, exposing agent IDs only when attributable to that session, and reporting approximate local process-tree RAM for the whole tab without blocking the UI.

#### Scenario: target tab and focused pane

- GIVEN a tab with multiple terminal panes
- WHEN its tab menu is opened
- THEN Tab ID identifies that tab, session copy actions identify its focused pane, and Debug Info includes every pane's Goko session ID and PID

#### Scenario: agent identity unavailable

- GIVEN a Claude, Codex, OpenCode, Devin, Antigravity, or Shell session
- WHEN the tab menu opens
- THEN an attributable agent conversation ID can be copied, otherwise the agent copy item is disabled with a reason; Shell has no agent conversation ID

#### Scenario: memory estimate

- GIVEN live sessions in a tab
- WHEN a worker sample is available
- THEN RAM sums unique local process RSS across its session trees, labels the value approximate, and excludes shared Goko UI/daemon and cloud memory; a missing sample is reported as unavailable rather than zero

Evidence: `apps/rune/src/tab_debug.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/session_debug.rs`
````

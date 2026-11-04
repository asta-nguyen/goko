# Project terminal workspaces, splits, and Vietnamese support tasks

## Design

[Project terminal workspaces, splits, and Vietnamese support](design.md)

## Global Constraints

- The daemon remains the sole PTY/process authority; terminal bytes never enter React state, Zustand, or the query cache.
- Tab and split layout is in-memory for the current app run. Project/session metadata remains in SQLite; no layout persistence or terminal snapshot is added.
- Each daemon session appears in at most one pane. A pane has one xterm controller, output subscription, and PTY resize source.
- Closing a pane/tab force-kills live sessions as soon as the daemon accepts stop for their state, then deletes daemon records and SQLite metadata only after exit is confirmed. Failed force-stop/delete leaves a visible retry path; never hide a possibly live process. The separate Stop control remains graceful.
- Daemon errors, including an older daemon rejecting `session.delete`, preserve the closing item and metadata; never stop or restart a daemon automatically.
- Removing a project does not stop sessions; open tabs are rehomed under Unassigned.
- The PTY child receives a UTF-8 locale without changing the app/daemon environment or app UI language. Use semantic tokens and accessible, keyboard-adjustable separators for the ProjectTree, Editor list/detail, and terminal splits.
- TypeScript remains strict; custom commands keep the existing argv/shlex validation path; automated tests never launch real agent CLIs.
- No new dependency or Tauri capability is added.
- ProjectTree and Editor list widths are UI-only, shared across views within the app run, and reset to their existing defaults after app restart.
- The active tab's complete terminal layout fills the available terminal workspace; all split panes remain visible and fit their allocated cells without changing stored ratios. Keep inactive tabs mounted and inactive. The app header, project sidebar, and tab bar stay visible; retain existing minimum pane sizes and scrolling when the viewport is too small.

## Impact map

```text
Entry: `apps/desktop/src/App.tsx::App`, `apps/desktop/src/features/editor/panes.tsx::SplitPane`, and `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::TerminalWorkspace`; PTY locale entry: `crates/daemon/src/pty.rs::PtyProcess::spawn`
Flow: App shell → ProjectTree/sidebar separator → main view; Editor Files/Changes/History/Stashes → shared SplitPane/list-detail separator; terminal panes → existing PaneTree/ResizableSplit; close control → TerminalWorkspace close state → `sessionService.stop(force=true)` → exit event/session query → `sessionService.delete` → query cache and `UiState.removeSession`; terminal sizing continues through TerminalController → terminal_resize
State changes: `UiState` gains in-memory ProjectTree and Editor list widths alongside active project and project-keyed tabs/split layouts; close retains its pane until force-stop exit and deletion succeed; no width persistence or database schema is added
External effects: pointer and keyboard resize the ProjectTree and Editor list/detail UI; changing the available terminal width uses the existing PTY resize path; closing a live pane sends SIGKILL through the daemon, then removes daemon replay/session and SQLite metadata after exit confirmation; child-only locale and terminal byte flows remain unchanged
Change candidates: `apps/desktop/src/components/ui/{resizable-split.tsx,resizable-split.test.tsx}` (new), `apps/desktop/src/{App.tsx,App.test.tsx,stores/ui.ts,stores/ui.test.tsx}`, `apps/desktop/src/features/editor/{panes.tsx,EditorWorkspace.test.tsx}`, `apps/desktop/src/features/terminal/{TerminalWorkspace.tsx,TerminalWorkspace.test.tsx}`, and `ARCHITECTURE.md`; no Rust or Tauri contract changes
Verification: `pnpm -C apps/desktop test`; `pnpm -C apps/desktop typecheck`; `pnpm -C apps/desktop build`; targeted tests cover immediate force-stop, exit-before-delete, retries, existing graceful Stop behavior, and ProjectTree/Editor pointer, keyboard, and bounds; the active change's final acceptance still runs `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo build --workspace`
Verified at: 25b30bdff7aacb54807131d93582f5e9e3d9ff8e
```

The trace uses the current working tree at the baseline commit above. Existing edits to `App.tsx`, `ProjectTree.tsx`, `ProjectTree.test.tsx`, `ARCHITECTURE.md`, and the S2 archive move predate this plan and remain user-owned.

## Tasks

### Task 1 — UTF-8 PTY locale and Vietnamese byte path

Files: `crates/daemon/src/pty.rs::PtyProcess::spawn`, `crates/daemon/tests/pty_spawn.rs`, `crates/daemon/tests/terminal_io_ipc.rs`, `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.mount`, `apps/desktop/src/features/terminal/TerminalController.test.ts`

Interfaces: preserves the existing xterm `onData` string and `Uint8Array` output channel; defines the child-only locale policy consumed by every PTY launch.

Change: Select a UTF-8 locale from inherited `LC_ALL`, `LC_CTYPE`, then `LANG`; when none is UTF-8, use `en_US.UTF-8`, clear a conflicting non-UTF-8 `LC_ALL` only in the child environment, and set child `LC_CTYPE`/`LANG` accordingly. Add focused round-trip coverage for `Tiếng Việt: ă â ê ô ơ ư đ` and decomposed combining marks. Do not normalize or translate terminal text.

Verify: `cargo test -p daemon pty` and `cargo test --workspace` — synthetic locale precedence/fallback resolves to UTF-8 and a real test PTY preserves the exact Vietnamese UTF-8 bytes; `pnpm -C apps/desktop test TerminalController` — `onData` preserves Vietnamese text and output bytes render as the same text.

Progress: done
Result: `cargo test -p daemon pty`, `cargo test --workspace`, and `pnpm -C apps/desktop test` passed; locale precedence/fallback and exact precomposed/decomposed Vietnamese UTF-8 bytes are covered.

### Task 2 — Delete only finished daemon sessions and metadata

Files: `crates/protocol/src/message.rs::Request` and tests, `crates/daemon/src/session.rs::SessionManager`, `crates/daemon/src/server.rs::handle`, `crates/daemon/tests/session_manager.rs`, `crates/daemon/tests/session_ipc.rs`, `apps/desktop/src-tauri/src/store.rs::Store`, `apps/desktop/src-tauri/src/commands.rs::Bridge`, `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src-tauri/src/lib.rs::handler`, `apps/desktop/src-tauri/src/wiring_tests.rs`, `apps/desktop/src/services/session.ts::sessionService`, `apps/desktop/src/services/session.test.ts`

Interfaces: adds idempotent `session.delete{id, session_id}` from the frontend bridge through Tauri and protocol to the daemon; adds an idempotent SQLite metadata delete keyed by the same session id.

Change: Reject deletion while a session is created, starting, running, stopping, or otherwise not finished. Cancel its output stream, delete `exited` and `failed` records from the daemon list and release their replay buffers, then remove project metadata. A missing daemon record counts as already deleted so retry can finish SQLite cleanup after a partial failure; other daemon errors leave metadata intact. A daemon that does not recognize the additive request leaves the close retryable and must be updated/restarted by the user; never stop a daemon automatically because it may own other live PTYs. No active process is stopped by the delete command.

Verify: `cargo test --workspace` — deletion of live sessions is rejected without signaling them; exited/failed sessions disappear from `session_list`; terminal output subscriptions stop; metadata and replay state are released; a repeated delete succeeds when the daemon record is already absent; a forced SQLite failure leaves a retryable cleanup state. A fake older-daemon `malformed` response must not delete SQLite metadata.

Progress: done
Result: `cargo test --workspace` passed; delete rejects live sessions, closes existing output subscriptions, removes exited/failed records and metadata, is retryable after SQLite failure, and preserves metadata on an older-daemon malformed error.

### Task 3 — Project workspaces, default tabs, and close lifecycle

Files: `apps/desktop/src/stores/ui.ts::UiState`, `apps/desktop/src/App.tsx::App`, `apps/desktop/src/App.test.tsx`, `apps/desktop/src/features/projects/ProjectTree.tsx::ProjectTree`, `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::TerminalWorkspace` (new), `apps/desktop/src/features/sessions/SessionControls.tsx::SessionControls`, `apps/desktop/src/services/session.ts::sessionService`, `apps/desktop/src/features/terminal/TerminalWorkspace.test.tsx` (new), `apps/desktop/src/features/projects/ProjectTree.test.tsx`

Interfaces: consumes project/session/agent queries and Task 2's `session_delete`; produces active-project workspace state, terminal tabs, and tab/pane close operations.

Change: Separate project activation from tree disclosure. Selecting a project with no open tabs creates one Shell tab; switching projects preserves each project's open tabs and active tab in memory. New Tab starts a Shell in the active project's root; agent/custom launches add their successful session as a tab. Selecting a project-tree session focuses its existing pane or opens it in a new tab. Stop leaves an exited pane inspectable; Close stops all live sessions in its target, keeps the target visible while stopping, then deletes each session and metadata only after that process exits. Remove each pane after its delete succeeds and the tab when all panes are gone. Keep retry/error state on stop/delete failure or daemon disconnect. Rehome open tabs under Unassigned when their project is removed without stopping their sessions.

Verify: `pnpm -C apps/desktop test TerminalWorkspace ProjectTree App` — default Shell is created once when activating a project with no tabs, including repeated activation while creation is pending; unavailable launches leave no phantom tab and daemon failures can be retried explicitly, selecting the already-active project does not duplicate an existing tab, closing the last tab leaves an empty workspace, reactivation creates a new default, switching away/back preserves open tabs, existing sessions focus/open once, closing a multi-pane tab stops every live session and removes each pane only after its exit and delete, Close waits for `exited` before `session_delete`, exited sessions skip Stop, older-daemon errors retain the closing item and metadata, and project removal preserves its PTYs under Unassigned.

Progress: done
Result: `pnpm -C apps/desktop test` passed; workspace activation, one default Shell, retry, project switching/removal, tab and pane close, exit-before-delete, and multi-pane cleanup are covered.

Historical behavior: this completed task implemented graceful Stop on Close. Task 6 supersedes that close behavior; the completed result above remains evidence for the earlier implementation.

### Task 4 — Nested splits with accessible live resizing

Files: `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::TerminalWorkspace`, `apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView`, `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.syncSize`, `apps/desktop/src/features/terminal/TerminalWorkspace.test.tsx`, `apps/desktop/src/features/terminal/TerminalController.test.ts`

Interfaces: consumes the workspace tab/pane state from Task 3 and existing FitAddon, ResizeObserver, and `terminal_resize`; each pane owns one distinct session id.

Change: Add Split Vertical (left/right, vertical separator) and Split Horizontal (top/bottom, horizontal separator) actions that start a new Shell in the same project and create a nested pane layout at 50/50. If session creation fails, retain the current layout and show the error without adding an empty pane. Add a feature-local separator that supports pointer drag and keyboard adjustment, exposes separator orientation/value to assistive technology, and clamps both sides to the minimum pane size. Keep xterm controllers mounted for open hidden tabs to retain output buffers; only visible pane resizes reach that pane's PTY. Closing a pane stops/deletes its session and collapses the layout after cleanup. No session is rendered in two panes.

Verify: `pnpm -C apps/desktop test TerminalWorkspace TerminalController` — split creates a distinct session with the same `projectId` and the requested orientation; pointer/keyboard resizing updates the correct PTY dimensions and clamps both sides to the same minimum ratio; hidden panes do not resize; duplicate session placement focuses the existing pane; closing a pane collapses only after session deletion.

Progress: done
Result: `pnpm -C apps/desktop test` passed; nested split directions, pointer and keyboard resizing with minimum bounds, hidden-pane resize suppression, and retry after a transient PTY resize error are covered.

### Task 5 — Fit the active session layout to the workspace

Files: `apps/desktop/src/features/projects/ProjectTree.test.tsx`, `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::PaneTree/ResizableSplit/TerminalWorkspace`, `apps/desktop/src/features/terminal/TerminalWorkspace.test.tsx`, `apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView`, `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.setVisible/syncSize`, `apps/desktop/src/features/terminal/TerminalController.test.ts`

Interfaces: consumes existing active-tab/focused-pane routing and per-pane `TerminalView` visibility; uses the existing FitAddon, ResizeObserver, and `terminal_resize` path without adding session state or IPC.

Change: When the project tree opens or selects a session, show its tab and focus its pane. Make the active tab's entire layout fill the available terminal content area below the workspace controls. A single pane uses the whole area; a split tree keeps all panes and separators visible and uses the whole area, with each terminal fitted to its assigned cell. Ensure each visible pane sends its own changed dimensions to its PTY. Keep inactive tabs mounted and inactive so they preserve their output subscriptions without sending resize requests. Workspace/window resizing must not alter stored split ratios. Keep the existing minimum pane size and scrolling behavior when the available area is smaller than the combined minimum. The full active split tree and app header, project sidebar, and tab bar remain visible.

Verify: `pnpm -C apps/desktop test ProjectTree TerminalWorkspace TerminalController` — selecting an existing Codex session focuses its tab and pane; the active tab panel fills its parent and every split pane remains rendered; an observed workspace resize fits each visible controller to its cell and sends the resulting dimensions to that pane's session; split ratios remain unchanged; inactive tabs remain mounted and do not resize. If the viewport is below the existing pane minimums, the current scroll behavior remains.

Progress: done
Result: The regression assertion failed before the fix because the single-pane wrapper lacked `flex-1`, then passed after the pane was allowed to grow. Fresh `pnpm -C apps/desktop test` passed (15 files, 134 tests); `pnpm -C apps/desktop typecheck` and `pnpm -C apps/desktop build` passed. Split panes remain active, the split root fills its tab panel, and workspace resize preserves the pane tree and ratios.

### Task 6 — Force-kill on pane or tab close

Files: `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::TerminalWorkspace`, `apps/desktop/src/features/terminal/TerminalWorkspace.test.tsx`, `apps/desktop/src/features/sessions/SessionControls.test.tsx`

Interfaces: uses the existing `sessionService.stop(sessionId, true)` and `sessionService.delete(sessionId)` paths from Tasks 2–3; does not change the daemon or Tauri contract.

Change: Closing a pane or tab requests force-stop for every live session as soon as the daemon accepts stop for its state. Keep the pane visibly Closing until its session reaches `exited` or `failed`, then delete its daemon record and SQLite metadata and remove the pane. A session still `created` or `starting` remains visible until it becomes stoppable or exits naturally, because the daemon rejects stop before a process is available. A naturally exited session skips force-stop. Failed force-stop or deletion stays visible with Retry; retry force-stops the session again if it is still live. Preserve graceful Stop and its existing delayed Force kill control in SessionControls.

Verify: `pnpm -C apps/desktop test TerminalWorkspace SessionControls` — closing a running or stopping pane calls stop with `force=true` immediately without the five-second delay; a created/starting pane remains visible until stop is accepted; natural exit skips stop; `session_delete` is not called until exit is confirmed; multi-pane tab close force-stops every live session and removes each pane only after its own exit and delete; failed force-stop and delete preserve a retryable closing item; retry repeats forced stop when needed; the separate Stop control remains graceful with Force kill offered after five seconds.

Progress: done
Result: `pnpm -C apps/desktop test TerminalWorkspace SessionControls` passed and fresh `pnpm -C apps/desktop test` and `pnpm -C apps/desktop typecheck` passed; close now sends `force=true` immediately for running and stopping sessions, created/starting panes stay visibly Closing until stoppable or naturally finished, exit-confirmed deletion and retryable force-stop/delete failures are covered, and SessionControls keeps graceful Stop with delayed Force kill.

### Task 7 — Resize ProjectTree and Editor panels

Files: `apps/desktop/src/components/ui/resizable-split.tsx::ResizableSplit` (new), `apps/desktop/src/components/ui/resizable-split.test.tsx` (new), `apps/desktop/src/App.tsx::App`, `apps/desktop/src/App.test.tsx`, `apps/desktop/src/stores/ui.ts::UiState`, `apps/desktop/src/stores/ui.test.tsx`, `apps/desktop/src/features/editor/panes.tsx::SplitPane`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

Interfaces: provides one reusable accessible splitter for the fixed ProjectTree and Editor list/detail panels; stores their current widths in the existing in-memory UI store. The Editor list width is shared by Files, Changes, History, and Stashes.

Change: Add pointer-drag and keyboard-adjustable separators with visible focus, separator orientation, and current/minimum/maximum values exposed to assistive technology. Keep the ProjectTree width's 320 px default, a 220 px minimum, and a 480 px maximum while keeping the main workspace at least 320 px. Keep the Editor list width's 288 px default, a 180 px minimum, and a 480 px maximum across all four sections and view switches while keeping the detail pane at least 320 px. Reduce each maximum with the available width; when the window cannot fit both minimums, retain them and allow horizontal scrolling. Keep both widths in memory only, resetting to these defaults on app restart. Leave the existing terminal split behavior intact.

Verify: `pnpm -C apps/desktop test ResizableSplit App EditorWorkspace` — initial widths match 320/288 px; pointer drag and keyboard arrows change the right width; Home/End and drag clamp to the same minimum/maximum while preserving a 320 px main/detail area; a narrow window reduces the maximum and scrolls without shrinking either minimum; the ProjectTree width survives Terminal/Editor switches; the Editor list width is shared across section and view switches. Existing terminal separator pointer/keyboard tests continue to pass.

Progress: done
Result: `pnpm -C apps/desktop test ResizableSplit App EditorWorkspace` passed and fresh `pnpm -C apps/desktop test` (19 files, 185 tests) and `pnpm -C apps/desktop typecheck` passed; the shared `ResizableSplit` keeps the 320/288 px defaults, clamps pointer and keyboard input to bounds that shrink to preserve each 320 px sibling minimum, keeps minimums and scrolls on narrow rows, and holds both widths in `UiState` across view and Editor-section switches while resetting on restart.

### Task 8 — Architecture handoff and manual desktop acceptance

Files: `ARCHITECTURE.md`; no protocol or UI code.

Interfaces: records the locale, tab/pane, full-workspace terminal sizing, resizable ProjectTree and Editor list/detail panels, force-kill close lifecycle, session-delete, and project-removal behavior for the next contributor.

Change: Update the architecture document to describe UTF-8 child locale handling, project workspaces, the tab-to-pane-to-session relationship, the active tab's full-workspace sizing for both single panes and complete split trees, resizable ProjectTree and Editor list/detail panels and their in-memory defaults, per-cell PTY dimensions, force-kill-on-close/exit-before-delete behavior, graceful Stop behavior, and the retry requirement when an older daemon does not support deletion. Record manual macOS observations for Telex/VNI input, paste/output, locale, opening a Codex session from the project tree, two tabs, both split directions filling the workspace, pointer/keyboard resizing of each panel, closing a live and exited session, retry after disconnect, and project removal with a running pane.

Verify: Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop typecheck`, and `pnpm -C apps/desktop build`. Manually run the listed macOS Tauri acceptance cases; confirm that opening a Codex session fills the available terminal workspace, a split layout keeps every pane visible and fills that workspace, the app shell remains visible, each terminal fits its own cell, all ProjectTree/Editor/terminal separators respond to pointer and keyboard input within bounds, shared panel widths survive view switches, and workspace resizing preserves split ratios; confirm closing a live session force-kills immediately and its record disappears only after exit; record each pass/fail/not-run result. IME behavior cannot be verified in jsdom.

Progress: in progress
Result: Fresh final checks passed: `cargo test --workspace` (204 passed), `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test` (19 files, 185 passed), `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, and `git diff --check HEAD`. Earlier isolated macOS acceptance passed for one Shell pane filling the terminal area, two tabs, nested left/right and top/bottom splits, selecting an existing session without duplication, and closing a split tab after cleanup. Manual acceptance of the new resizers and force-kill-on-close remains not run: CUA exposed only the existing `dev.goko.app` window, which contained active user sessions, so no controls were changed; the isolated dev server and its temporary daemon were stopped and its temporary home removed. Still not run: Telex/VNI input and Vietnamese paste/output, child `locale charmap`, launching Codex from the project tree, pointer/keyboard/window resizing in the native app, closing an already-exited session, retry after daemon disconnect, and project removal with a running pane. The Codex CLI and native input-method behavior were unavailable in this check.

## Files inspected, no change

- The following files provide context for the added panel and close behavior.
- `apps/desktop/src/stores/ui.ts::openSession` — existing active-tab/focused-pane routing consumed by the sizing change; no new state is required.
- `apps/desktop/src/services/session.ts` and `apps/desktop/src/features/sessions/queries.ts` — existing stop/delete services and query-cache updates; no bridge change is planned.
- `apps/desktop/src/features/projects/ProjectTree.tsx` — sidebar contents remain the same; its outer width is controlled by `App`.
- `apps/desktop/src/features/editor/EditorWorkspace.tsx` and `ViewSwitch.tsx` — Editor sections and Terminal/Editor remain navigation tabs; only the shared list/detail boundary is resizable.
- `apps/desktop/src/features/editor/{EditorFiles.tsx,EditorChanges.tsx,EditorHistory.tsx,EditorStashes.tsx}` — all four screens consume the shared `SplitPane`; they need no per-screen resizing implementation.
- `apps/desktop/src-tauri/src/{commands.rs,terminal.rs,store.rs}` — Tauri bridge, per-session output subscription, and session metadata retention.
- `crates/protocol/src/message.rs`, `crates/daemon/src/{pty.rs,session.rs,server.rs}`, and `crates/daemon/tests/{pty_io.rs,terminal_io_ipc.rs,session_manager.rs}` — PTY locale/resize, session lifecycle, byte transport, and available test patterns.
- `docs/agent-devkit/changes/archive/2026-10-30-s2-projects-agents-sessions/{design.md,tasks.md}` and `docs/agent-devkit/roadmap.md` — current project/session contract and S4 reattach boundary.
- `docs/llm/AGENTS.md` and `docs/llm/INDEX.md` — neither exists; no wiki coverage or delta is applicable.

## Approval Gate

Status: approved

## Decision Log

### D1 — Close stops and fully deletes after process exit

Decision: Closing a terminal pane/tab requests Stop, waits until its process has exited, then deletes the daemon session/replay data and project metadata. An active process is never deleted. Confirmed by user: 2026-11-02.

### D2 — Maximize the focused pane within its workspace

Decision: Maximize the selected pane while keeping the app header, project sidebar, and tab bar visible. Keep per-tab maximize state in memory, preserve split tree/ratios, and leave hidden sibling sessions mounted and inactive. This revision was superseded by D3, which keeps the complete active split layout visible; D3 was approved by the user on 2026-11-02.

### D3 — Fill the complete layout for the selected session

Decision: When opening/selecting a Codex session, activate its tab and focus its pane. Fill the available terminal workspace with the complete active-tab layout: one pane fills it alone; a split keeps every pane visible and fits each to its cell. Keep sidebar, app header, and tab bar visible. This clarification supersedes D2's pane-only hiding behavior. Approved by the user with “fix đi” on 2026-11-02.

### D4 — Force-kill sessions when closing a pane or tab

Question: What should Close do to a live terminal session?
Decision: The user requested that closing a terminal panel force-kill its session and delete it immediately. The approved plan requests force-stop as soon as the daemon accepts it, waits for confirmed exit before using the existing delete path, and keeps failed cleanup visible and retryable.
Impact: Supersedes D1 and Task 3's graceful-stop-on-close behavior; adds Task 6 and updates architecture/manual acceptance. The daemon's existing refusal to delete live sessions remains unchanged.
Confirmed by user: 2026-11-03 — approved with the revised design and task plan.

### D5 — Resize existing app and Editor panel boundaries

Question: Which current panels need resize handles?
Decision: The user requested resizing between screens and menu panels. The approved plan applies that to the fixed ProjectTree sidebar and the shared Editor list/detail boundary; terminal split separators already resize. The design uses existing widths as defaults and keeps new widths in memory for the app run.
Impact: Adds Task 7 and updates the architecture/manual acceptance; no project-tree contents, tab navigation, database schema, or dependency changes.
Confirmed by user: 2026-11-03 — approved with the revised design and task plan.

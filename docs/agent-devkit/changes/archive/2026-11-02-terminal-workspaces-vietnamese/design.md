# Project terminal workspaces, splits, and Vietnamese support

## Goal

Make each registered project a terminal workspace with a default shell tab, additional terminal tabs, and resizable panels. The project sidebar and Git editor list/detail panels can be resized; terminal split panes already support resizing. Closing a terminal pane or tab force-kills its live session and deletes it after the daemon confirms exit. The active session layout fills the available terminal workspace, whether it contains one pane or a split tree. Ensure Vietnamese text can be composed, sent to a process, and rendered correctly in the terminal.

## Scope

In:

- Support Vietnamese terminal input, output, and paste through xterm, Tauri, Rust, and the PTY. Set a UTF-8 locale for each PTY child without changing the app UI language or hardcoding a Vietnamese locale.
- Preserve the existing strict TypeScript, semantic colour tokens, shell-command validation, and rule that terminal bytes stay out of React state. Automated tests use fake services or shell processes; they do not launch real agent CLIs.
- Selecting a project activates its workspace. If it has no open terminal tab, create one Shell tab. Switching away and back within the app restores that project's tabs and active tab.
- A new tab starts a distinct Shell session in the same project. Starting a discovered agent or custom command from that project opens the created session in a new tab. Selecting an existing session in the project tree opens it in a tab or focuses its already-open pane.
- A tab contains one terminal pane or a nested split layout. Splitting a pane starts a distinct Shell session in the same project. Support horizontal and vertical splits, pointer resizing, and keyboard-adjustable separators.
- Make the app's ProjectTree sidebar and the shared Editor list/detail column resizable with pointer drag and keyboard controls. Preserve their current widths as defaults (320 px and 288 px). Bound the ProjectTree to 220–480 px and keep the main workspace at least 320 px wide; bound the Editor list to 180–480 px and keep its detail pane at least 320 px wide. Reduce each maximum to preserve its sibling minimum; if the window is narrower than the combined minimums, keep the minimums and allow horizontal scrolling. Keep both widths in UI memory across view and Editor-section switches; widths reset to defaults when the app restarts.
- The active tab's complete terminal layout fills the available terminal workspace; all split panes remain visible and fit their allocated cells without changing stored ratios. Keep inactive tabs mounted and inactive. The app header, project sidebar, and tab bar stay visible; retain existing minimum pane sizes and scrolling when the viewport is too small.
- Closing a pane or tab immediately force-kills each live PTY session in the target as soon as the daemon accepts stop for its state. The daemon rejects stop while a session is `created` or `starting`, so keep it visibly Closing until it becomes stoppable or exits naturally. Keep the closing UI visible until the daemon confirms each process exited, then delete its daemon session record, replay buffer, and SQLite metadata. A close racing with natural exit skips the kill. Force-kill or deletion failures keep the closing item visible with a retry action. The separate Stop control remains graceful and keeps its existing delayed Force kill action.
- Keep existing project-removal behavior: removing a project does not stop its sessions. Rehome its open tabs under Unassigned so they remain reachable.

Out:

- Translating Goko's interface, project settings screens, terminal search, persistent tab/layout restoration, daemon restart recovery, and terminal-screen snapshots. Existing sessions remain listed by the daemon and can be opened manually; this work does not promise a restored tab layout or screen after app restart.
- Showing one daemon session in multiple panes. A session has one terminal subscription and one PTY size, so each pane owns a distinct session.
- New terminal-rendering dependencies. Use the installed xterm and FitAddon; keep the existing terminal split behavior.
- Resizing between Editor sections as if they were simultaneous panels; Files, Changes, History, and Stashes are tabs and share the same resizable list/detail width.

## Behavior

**Vietnamese text.** xterm's `onData` string passes through the existing Tauri string command and is encoded as UTF-8 for the PTY. PTY output remains bytes and is written to xterm as a `Uint8Array`. For each child process, choose the first valid UTF-8 locale from `LC_ALL`, `LC_CTYPE`, then `LANG`. If none is valid, use `en_US.UTF-8`; remove a conflicting non-UTF-8 `LC_ALL` in that child and set `LC_CTYPE` and `LANG` to the selected locale. Do not alter the daemon's or app's environment. Vietnamese Telex/VNI composition is tested in the macOS Tauri app because jsdom cannot verify the native input method.

**Project workspaces.** Project-row selection activates the workspace; a separate disclosure control expands or collapses its session children. The first activation with no tabs creates one Shell session and tab. Repeated activation while that launch is pending must not create duplicate sessions. Later project switches restore its in-memory tab layout and active tab. Closing the last tab leaves an empty workspace until the user creates a tab or leaves and reactivates the project; selecting the already-active project does not spawn a replacement. The layout is not persisted across app restarts. Existing child-session rows remain the session navigator: selecting one focuses it if already open, or opens it in a new tab.

**Tabs and panes.** The tab bar belongs to the active project. New Tab starts a fresh Shell PTY in that project's root. Agent and custom-command launches continue to use the project's existing launch menu and add the successful session as a tab. Split Vertical creates left/right sibling panes with a vertical separator; Split Horizontal creates top/bottom panes with a horizontal separator. Each starts at a 50/50 ratio, and further splits can nest. A session id may appear in only one pane. Keep open xterm instances mounted while their tabs are hidden so each pane retains its own scrollback while its process continues; hidden panes do not resize their PTYs. The visible pane's existing FitAddon and ResizeObserver path sends debounced size changes to its own session.

**Project and Editor panel resizing.** The ProjectTree sidebar keeps its current 320 px default and can be resized from 220 to at most 480 px while the main workspace retains at least 320 px. The shared Editor list/detail split keeps its current 288 px default and can be resized from 180 to at most 480 px while the detail pane retains at least 320 px. Each maximum shrinks with the available width; if a narrow window cannot fit both minimums, retain the minimums and allow horizontal scrolling. Both separators support pointer dragging and keyboard adjustment and expose their current and bounded sizes to assistive technology. Widths remain in UI memory while switching workspaces, views, or Editor sections, and reset on app restart. Existing terminal split separators keep their current behavior.

**Closing and stopping.** Stop in SessionControls requests graceful process termination and leaves the exited tab available for output inspection; its existing Force kill action remains available after five seconds. Closing a pane or tab is the destructive action: request Force kill for every live session in the target as soon as the daemon accepts stop for its state, keep the target in a visible Closing state, and wait for daemon events to report `exited` or `failed`. The daemon rejects stop while a session is `created` or `starting`; leave those items visibly Closing until they become stoppable or exit naturally. Delete each session record and its SQLite metadata only after that session exits; remove its pane after deletion succeeds, and remove the tab when all its panes are gone. A session already finished skips Force kill. If force stop, delete, storage, or daemon connectivity fails, retain the closing item with an actionable error and retry path; retry force-stops a still-live session again before deletion.

An already-running daemon from an older app build may not recognize `session.delete`. In that case, keep the closing item and SQLite metadata visible, surface the daemon error, and allow retry after the daemon binary is updated and restarted. Never stop or restart the daemon automatically because it may own other live PTYs. Treat a missing daemon session as already deleted so a retry after partial cleanup can finish deleting SQLite metadata.

**Project lifecycle.** Removing a project does not stop its PTYs. Its open tabs move to Unassigned with the sessions, which remain closable there. A missing project root disables new tabs, agent launches, and splits; existing panes remain viewable and closable.

If the daemon is disconnected or the project root is missing when the first Shell tab would be created, keep the workspace empty and show the launch error; do not add a phantom tab. Offer Retry after a daemon launch failure. A missing root stays blocked until the registered directory exists again.

**Fit the active session layout.** Selecting or opening a session from the project tree activates its tab and focuses that session's pane. The complete active tab layout fills the terminal content area below the existing workspace controls. A single pane fills the area; a split tree keeps every pane and separator visible while its root fills the area, and each terminal fits its own pane cell. Use the existing FitAddon and ResizeObserver path so active panes send their cell dimensions to their own PTYs. Keep inactive tabs mounted and inactive; they do not send resize requests. Workspace and window resizing do not change stored split ratios. Respect the existing minimum pane size; if the available area is smaller than the combined minimum, retain the existing scroll behavior. Keep the app header, project sidebar, and tab bar visible, with the complete active split tree rendered. Native window fullscreen is outside this change.

## Edge cases

- An inherited non-UTF-8 `LC_ALL` overrides other locale variables on Unix. Clear it only in the PTY child when selecting a UTF-8 fallback; leave the daemon and app environment untouched.
- Test both precomposed Vietnamese characters and decomposed base-plus-combining sequences. Test typing through Telex/VNI, paste, and text emitted by a child process.
- A split, tab switch, or project switch must not stop another PTY or route output/input to a different session.
- A session already open in a pane cannot be opened in another pane. Selecting it focuses its existing tab/pane instead of creating a duplicate subscription or competing resize source.
- Closing a tab with multiple panes force-kills every live pane session. Delete each session only after it exits; keep the tab Closing until every pane is deleted and retain any failed cleanup for retry.
- A close request racing with natural process exit skips Force kill and proceeds to deletion. A running or stopping session cannot be deleted by the daemon API.
- If creating a new tab or split session fails, keep the current workspace layout unchanged and show the launch error; never leave an empty placeholder pane.
- If a first-tab Shell launch fails because the daemon is disconnected or the root is missing, keep the workspace empty. Do not retry automatically; a Retry action after disconnect or restoring the root can try again.
- If the daemon disconnects before exit or delete confirmation, retain the Closing item and metadata. Retry after reconnection; never infer process exit from a failed IPC call.
- If daemon deletion succeeds but SQLite metadata deletion fails, retry is idempotent: a missing daemon record is treated as already removed and the metadata deletion is retried.
- If an older daemon does not recognize `session.delete`, do not treat its malformed/unsupported-request response as deletion success or remove SQLite metadata; retain the closing item and retry after that daemon is updated and restarted.
- A project removed while a tab is open moves the tab to Unassigned without stopping its sessions. A missing root blocks new PTYs but does not stop existing ones.
- A narrow pane cannot be resized below the minimum size for either sibling; keyboard and pointer resizing report the same bounded ratio.
- Selecting a project-tree session focuses its existing pane or opens it in a tab, then the active tab's entire layout fills the available terminal workspace. Every pane in a split remains visible and uses only its own cell for PTY resizing.
- Resizing the workspace/window fits the active terminal pane(s) to their available cells without changing stored split ratios or stopping, deleting, duplicating, or rerouting sessions. Inactive tabs remain mounted but do not resize.
- The application header, project sidebar, and tab bar remain visible. The existing minimum pane size remains in force; a window smaller than the combined minimum can scroll.
- The ProjectTree and Editor list/detail separators enforce their respective minimum and maximum widths for both pointer and keyboard input. Their sizes are shared across workspaces and Editor sections for the app run and reset on restart.

## Interfaces

- `UiState` gains an active project, project-keyed terminal workspace state, and in-memory ProjectTree and Editor list widths. Each tab stores a pane-layout tree; each pane references one unique daemon session id. No SQLite layout schema or panel-size persistence is added.
- `App` renders the active project's `TerminalWorkspace`; project selection is separate from the tree disclosure state. `SessionControls` follows the focused pane.
- `TerminalWorkspace` owns tabs, split actions, close lifecycle, project-to-tab routing, and fills its available content area with the active tab's complete layout. Close force-stops live sessions and waits for exit before calling the existing session-delete path. `TerminalView`/`TerminalController` remain one xterm instance per pane/session, fit visible panes to their cells, and continue to own output outside React state; inactive tabs remain mounted and inactive.
- `App` and the shared Editor `SplitPane` use an accessible pointer- and keyboard-adjustable separator for their respective ProjectTree and list/detail widths. Existing terminal split separators remain unchanged.
- `session_create` remains project-scoped. A new `session.delete` protocol request carries the standard request `id` and `session_id`; the daemon cancels any output stream and removes only finished sessions. The Tauri `session_delete` command also deletes their `sessions` metadata rows. A missing daemon record is already deleted for retry purposes; other daemon errors retain metadata. The operation is idempotent after partial cleanup.
- PTY startup resolves a UTF-8 child locale. The terminal byte channel, project metadata schema, and existing `session_create`, `terminal_write`, `terminal_resize`, and output-subscription shapes remain unchanged.
- No new dependencies, Tauri capabilities, or UI localization settings are required.

## Verification

- Rust tests cover UTF-8 locale precedence and fallback, Vietnamese bytes through PTY input/output, rejection of deletion for live sessions, successful removal of exited/failed sessions from daemon listing, release of their replay buffers, and idempotent delete retries.
- Desktop tests cover Unicode input/output through `TerminalController`, project activation and default Shell creation, project tab preservation while switching, opening/focusing existing sessions, close force-killing live sessions before exit-confirmed deletion, retained closing state and retry after failures, split session uniqueness, per-pane resize routing, keyboard-accessible terminal separators, ProjectTree and Editor list/detail pointer/keyboard resizing with bounds, and fitting a single pane or complete split tree to the available workspace while preserving ratios and inactive-tab sessions.
- Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop typecheck`, and `pnpm -C apps/desktop build`.
- Manual macOS Tauri check: `locale charmap` in a new terminal reports UTF-8; type and paste `Tiếng Việt: ă â ê ô ơ ư đ` with Telex and VNI; echo the same text; verify precomposed and decomposed text; create two tabs and a horizontal/vertical split in one project; open a Codex session from the project tree and confirm its single-pane layout fills the available workspace; confirm a split layout keeps every pane visible and fills that workspace; drag and keyboard-resize the ProjectTree, Editor list/detail, and terminal separators; resize the window and confirm each active terminal fits its cell without changing split ratios; switch projects repeatedly; close live and already-exited panes; verify live sessions are force-killed, records disappear only after exit, and close errors remain visible through disconnects.
- Manual IME behavior cannot be established by jsdom. The repository has no macOS WKWebView automation driver, so record those input-method observations in the task result.

## Related context

The current app layout and session close contract in [ARCHITECTURE.md](../../../../ARCHITECTURE.md). `docs/llm/` is absent.

## Previous work

- [S2 Projects, agents, and sessions design](../archive/2026-10-30-s2-projects-agents-sessions/design.md) and [tasks](../archive/2026-10-30-s2-projects-agents-sessions/tasks.md) — registered projects, daemon-owned sessions, project/session metadata, and the current project tree.

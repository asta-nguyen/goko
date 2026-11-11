# Rune UX improvements: first sessions, safety, shortcuts, accessibility

## Goal

A new user opens Rune, sees what to do, starts an agent in two clicks, and cannot lose uncommitted work by a stray click. Today the Terminal view opens to a line of grey text, starting an agent needs a hover-only `+`, **Remove worktree** force-removes a checkout without asking, the app has no tab or settings shortcuts, and 16 sidebar controls have no accessible name. This change fixes those in the existing chrome and editor shell without a visual redesign.

## Scope

In (one change, ten tasks, in the order listed in `tasks.md`):

- **Accessibility and help text:** every sidebar control has a name that includes its target; tooltips explain the dirty dot, the `missing` badge, truncated labels, and the usage chips' wording.
- **Notices:** the global notice toast closes by itself.
- **First run and flow:** the empty Terminal view offers start buttons; the tab bar's New tab gains an agent menu; unavailable agents say why.
- **Safety:** **Remove worktree** and **Remove project** ask for confirmation.
- **Shortcuts:** `⌘T`, `⌘1`–`⌘9`, `⌘,`.
- **Connection:** the disconnected header pill shows the reason and that it is reconnecting.
- **Sidebar noise:** a project's More menu gains **Prune missing worktrees**.
- **Editor:** the empty Editor lists recent projects; the Editor's notice bar can be dismissed.

Out (each needs its own design; listed so they are not lost):

- Multiple open files, file tabs, back/forward, and recent files in `⌘P` (the editor holds one `doc` today).
- Git Pull, Fetch, branch switch/create, and a non-fast-forward push recovery path.
- Merging the Editor's project/checkout picker bar with the sidebar selection.
- Editor font size, word wrap, autosave; terminal search; a command palette.
- Confirmation for Stop and Close pane (changes `RNC-session-actions`; decide separately).
- Hiding third-party temporary worktrees (for example `.orca-preparing`): needs a rule that is not tool-specific.
- Workspace grouping, collapse-all, and a project filter: owned by [Multi-project workspaces](../archive/multi-project-workspaces/design.md). This change adds no requirement it owns and edits no requirement ID it targets. Both change `sidebar.rs`, so implementation is sequential: start this change's sidebar tasks after that change's Task 3 is done or its sidebar edits are merged.
- Any change to the daemon, the protocol, `goko.db` schema, or visual theme.

## Behavior

- **Names.** Chevron and disclosure buttons are named "Collapse <name>" / "Expand <name>". Row actions are named "New session in <row name>", "More actions for <row name>", "Stop session <title>", "Close pane <title>". No sidebar button is unnamed.
- **Help text.** A hover tooltip says: a warning dot = "Uncommitted changes"; a muted dot = "Git status unknown"; the `missing` badge = "This folder no longer exists on disk"; a truncated project, worktree, or session label shows its full text; a usage chip shows what its percentage measures (used share of that window).
- **Notice toast.** The toast closes after 8 seconds and still has its close button. A newer notice replaces an older one and restarts the timer.
- **Empty Terminal view.** With no open tab and at least one project, the area shows one primary button per available agent plus **Shell**, targeting the active editor checkout, else the first project in the sidebar list. With no projects it shows **Add Project**. Buttons are disabled with a reason while the daemon is disconnected.
- **New tab menu.** New tab keeps starting a shell. A chevron beside it opens a menu with each agent preset and **Custom command…**, targeting the active checkout, using the sidebar menu's entries and launch path. An unavailable agent stays disabled and names its reason, "`<binary>` not found on PATH", in the sidebar menu and here.
- **Confirmation.** **Remove worktree** opens a modal naming the folder, saying whether it has uncommitted changes (from the row's `dirty` value), and how many sessions run in it; confirming removes it as today (force, because the user confirmed), cancelling does nothing. Running sessions are not stopped. **Remove project** opens a modal saying only the registration is removed, its sessions become Unassigned, and files stay. Escape and Cancel dismiss; one confirmation is open at a time.
- **Shortcuts.** `⌘T` starts a shell as New tab does. `⌘1`–`⌘9` activate that tab in the Terminal view; a number past the last tab does nothing. `⌘,` toggles Settings through the existing draft guard. They work in Terminal and Editor views, not while the sidebar prompt, a confirmation, the draft guard, quick-open, or an IME composition owns input. `⌘W` is untouched: it is window close.
- **Connection.** While disconnected the header pill reads "Reconnecting…" with a tooltip carrying the daemon's last reason; the connected state is unchanged. There is no Retry button: the daemon client already redials with capped backoff.
- **Prune missing worktrees.** A git project's More menu offers **Prune missing worktrees** when at least one of its worktrees is `missing`; choosing it runs `git worktree prune`, which removes only Git's records of folders that no longer exist, then refreshes the list. No folder or session is touched.
- **Editor.** With no checkout selected, the empty Editor lists up to five registered projects (name and root) that open that project's root checkout through the existing draft-guarded selection, beside **Add project**. Its notice bar gains a dismiss button.

## Edge cases

- Confirmation while the daemon is disconnected: Remove worktree/project need no daemon and stay available; the failure path is the existing notice.
- A confirmation target that disappears (list refresh removed it) while the modal is open: confirming reports a notice "That item no longer exists" and removes nothing.
- The draft guard and a sidebar confirmation never stack: the guard wins; a confirmation is requested only while no guard is open.
- Removing the worktree that is the editor's active checkout keeps the existing stale-checkout handling.
- `⌘T` with no active checkout starts the shell in `$HOME`, as New tab does today.
- `⌘1`–`⌘9` while the Editor view is active switch to the Terminal view only when that tab exists; otherwise they do nothing.
- Prune with Git failing (not a repo, locked) leaves the list unchanged and shows the error notice.
- Notice auto-expiry must not hide a notice that arrived less than 8 seconds ago; each new notice restarts the timer.
- No agent is available: the empty Terminal view still offers **Shell**.

## Interfaces

- `apps/rune/src/sidebar.rs`: names, tooltips, remove confirmations, prune menu item, agent-unavailable reason.
- `apps/rune/src/chrome.rs`: New tab menu, usage-chip wording, disconnected pill.
- `apps/rune/src/panes.rs::terminal_workspace`: empty-state actions.
- `apps/rune/src/main.rs`: notice expiry, shortcut handling in `on_key`, `State` confirmation field.
- `apps/rune/src/editor/workspace.rs`: empty Editor projects list, notice dismiss.
- `apps/rune/src/editor/backend.rs`, `backend/apply.rs`, `crates/app-core/src/worktrees.rs`: one new request, prune.
- `docs/llm/domains/rune-chrome.md`, `rune-editor.md`: new requirements per `delta.md`.

## Verification

Focused Rune view tests per task, then `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`. Accessibility names, confirmations, shortcuts, and the empty states are also checked in the real app by reading its accessibility tree and driving it (`agent-desktop` on a throwaway `GOKO_HOME`, never the user's running instance) when the app can launch; if not, the limit is recorded in the task result.

## Related context

- [Rune chrome](../../../llm/domains/rune-chrome.md)
- [Rune editor](../../../llm/domains/rune-editor.md)

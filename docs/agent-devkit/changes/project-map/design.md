# Project Map

## Goal

Show which registered projects belong to each workspace and let the user jump to a project without manually switching groups and finding its sidebar row.

## Scope

- A native Rune Project Map dialog, opened with plain `⌘K` or an accessible Project Map button beside the sidebar workspace selector, in Terminal and Editor.
- A workspace-to-project hierarchy with search, pointer selection, keyboard navigation, current-selection markers, and sidebar reveal.
- Reuse registered projects, workspace membership, selected-workspace persistence, the checkout navigation guard, theme tokens, and existing modal/input patterns.
- No database, daemon, protocol, dependency, workspace-membership, or project-file changes. No graph canvas, pan/zoom, drag-and-drop, recent-project history, worktree/session nodes, or terminal-session activation.
- This request produces a plan only. Application implementation requires approval of these written files.

## Behavior

- The map is a centered, bounded modal above the existing app, with a labeled search input, close action, and vertically scrollable hierarchy. Workspace headings are clickable and projects are indented beneath them with subtle connector lines. All groups are expanded initially; no collapse controls. Reuse Rune chrome typography, icons, spacing, light/dark tokens, and reduced-motion behavior.
- Use `State.workspaces` and `editor.projects` as the source, keyed by stable IDs. Keep their existing order. Show each project once under its `workspace_id`, including Default. Empty groups remain visible for an empty query. Show the full project path as secondary text/help so duplicate names remain distinguishable.
- Search trims and matches case-insensitive substrings against workspace name, project name, and root path. A matching workspace name shows all its projects; otherwise show only matching projects and their parent workspace headings. Whitespace-only query shows the whole map. Show a distinct no-results message.
- Opening focuses an empty search input; the first visible selectable row is the keyboard cursor. Up/Down moves across workspace and project rows in displayed order without wrapping; Enter activates the cursor. Moving the cursor scrolls its row into view. Tab reaches the search and close controls. Escape, the close button, or clicking the scrim dismisses without changing selection. Restore the previous usable input focus on dismissal, or transfer focus to the draft guard when one opens.
- The selected sidebar workspace and committed project root have separate text/icon markers; they can refer to different workspaces. An active linked worktree marks its parent project, without adding worktree nodes. Keyboard cursor styling is distinct from committed selection.
- Clicking a workspace heading calls the existing selected-workspace path, remembers it using `rune.selected_workspace`, closes the map, and preserves editor checkout, view, tabs, and sessions.
- Clicking a project selects its root checkout, switches the sidebar to the project's owning workspace, closes the map, and reveals the project row. Keep Terminal/Editor unchanged. In Terminal, retain all tabs, pane layouts and processes, show only the active project’s tabs and sessions, and restore its remembered tab/pane (or show the start screen when it has no tab); the newly chosen checkout is available to subsequent actions such as New tab. In Editor, load the selected root using the existing checkout refresh path. The same project-scoped terminal selection applies through the sidebar and editor project picker.
- Treat project selection as one guarded navigation intent. Workspace selection, checkout identity, highlight, and reveal happen only after the existing Save/Discard/Cancel guard permits the switch. Cancel or failed save preserves the previous workspace, checkout, document, tabs, sessions, and selection setting. The map closes before the guard is shown, avoiding overlapping modals.
- Plain `⌘K` opens only in Terminal/Editor, while no prompt, confirmation, picker, menu, guard, quick-open, or terminal IME composition owns input. Additional Ctrl/Shift/Option modifiers do not trigger it. While open, the map owns typing, paste, navigation keys, and shortcuts; none reaches a terminal or underlying editor. Preserve `⌘P`, system close/quit behavior, and existing guarded navigation. Settings does not open the map.
- Selections are ephemeral except for the existing selected-workspace setting. Do not persist the map query, keyboard cursor, or project history.

## Edge cases

- Loading, failed reads, empty registered-project list, empty workspace, and no search results have distinct labels. Existing read errors remain visible; retry uses existing workspace/project refresh requests. Do not fabricate successful empty data. Disable selection until both initial lists are available; cached rows can remain visible with an error indicator.
- Recompute displayed rows from current data when replies arrive. Preserve the cursor by stable row identity when it still exists; otherwise select the first visible row. Duplicate names use distinct IDs and accessible names including workspace/path.
- Resolve project ID, current root, and owning workspace again at activation and after any draft-guard delay. A moved project selects its current owner. A removed project or missing owner closes the attempted selection with a notice and changes neither workspace nor checkout. Preserve existing backend path/checkout validation; the map does not authorize arbitrary paths.
- Workspace clicks also recheck the ID; a removed workspace produces a notice without mutating selection. Repeated activation during a draft guard is ignored by the existing modal guard.
- Selected-workspace persistence failure keeps the existing in-memory navigation behavior and surfaces the existing failure notice. A late startup restore must not overwrite an explicit map choice.
- After a successful project switch, scroll the keyed sidebar row into view after its destination workspace has mounted, including when the row was below the viewport. Do not use an index derived from a stale pre-switch layout.
- Long paths/names truncate with full accessible labels/help. Large lists scroll inside the dialog; narrow windows keep input, actions, and rows within available bounds. Both themes retain visible focus and text/icon selection cues.

## Interfaces

- New internal `apps/rune/src/project_map.rs`: modal state, stable workspace/project row identities, filtering, keyboard/pointer handling, and modal layers. This module exists to contain the requested surface; use existing `TextEdit`, rune-kit fields/dialog primitives, and Quick Open patterns without introducing a generic palette abstraction.
- `apps/rune/src/main.rs::State`, `view`, `on_key`, `app_shortcut`, root paste routing: own the map, mount its layers, block underlying input, and expose the shortcut. Existing `State::select_workspace` remains the persistence path.
- `apps/rune/src/sidebar.rs::workspace_header` adds the map button without removing the current workspace menu. Give the scroll container a stable key and retain the existing `sidebar-project:<id>` row keys for reveal using `Tx::scroll_into_view_in`, already used by editor navigation. Schedule reveal after the new list mounts.
- `apps/rune/src/editor/navigation.rs::PendingNav` adds an internal `SelectMappedProject { project_id }` intent. Its guarded apply re-resolves the registered project and owning workspace, then selects the workspace and root checkout as one UI transition. Other callers retain `SelectProject`/`SelectCheckout` semantics.
- `docs/llm/domains/rune-chrome.md` gains the Project Map requirement from the sibling delta after implementation. Existing `RNC-project-workspaces` remains the workspace-only contract; existing `RNE-checkout-selection` remains the checkout guard/validation contract.

## Verification

- Unit tests: complete hierarchy, membership, Default/empty groups, duplicate names/IDs, case-insensitive name/path/workspace filtering, whitespace query, no results, cursor preservation/clamping, stale removal/move, and loading/error states.
- Rune runtime tests: shortcut and button from Terminal/Editor; suppression from Settings, IME, modified chords and modal owners; typing/paste/shortcuts cannot reach underlying surfaces; Escape/scrim/close, pointer/keyboard selection, focus and row visibility, both theme trees and narrow dialog bounds.
- Guard/navigation tests: cross-workspace project choice succeeds after Discard or successful Save; Cancel and failed Save leave workspace, setting requests, checkout, document, and terminal state unchanged; stale project removed/moved during guard resolution; workspace-only click preserves checkout; startup restore cannot replace a map selection; persistence enqueue failure shows its notice.
- Before source edits run relevant existing Rune/navigation tests and record baseline failures. Final commands: `cargo fmt --check`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`.
- Native inspection through `apps/rune/run-macos.sh debug`: real cross-workspace map selection, below-viewport sidebar reveal, draft guard, keyboard/paste isolation, focus restoration, narrow window, and both themes. Runtime tests cannot establish rendered native appearance; record any unavailable inspection as a verification limit.

## Related context

- [Rune chrome](../../../llm/domains/rune-chrome.md)
- [Rune editor](../../../llm/domains/rune-editor.md)

## Previous work

- [Multi-project workspaces](../archive/multi-project-workspaces/design.md) established persisted membership and workspace-only sidebar filtering.
- [Rune UX improvements](../archive/ux-improvements/design.md) established modal shortcut ownership, hidden-workspace attention, and first-launch actions.

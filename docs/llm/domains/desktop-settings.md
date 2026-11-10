# Desktop Settings

## Requirements

### SET-acp-agents

The system SHALL list five preset ACP agents (Devin, OpenCode, Claude Code, Codex, Gemini) in a Settings section where each can be enabled or disabled, given a command line and a commit prompt, and connected or disconnected with a visible status, saving overrides immediately and rejecting an invalid command line.

#### Scenario: Change an agent command

- GIVEN the ACP agents section is open
- WHEN the user edits the Devin command line to a valid value
- THEN the change is saved and the next Generate uses it

#### Scenario: Connect and disconnect

- GIVEN an enabled agent is disconnected
- WHEN the user presses Connect and later Disconnect
- THEN its status becomes connected and then disconnected, and Disconnect ends the agent process

#### Scenario: Reject an invalid command

- GIVEN the ACP agents section is open
- WHEN the user enters a command with an unterminated quote
- THEN an inline error is shown and the stored value is unchanged

Evidence: `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/settings/AcpAgentsSection.tsx`, `apps/desktop/src-tauri/src/acp.rs`, `apps/desktop/src/features/settings/SettingsPage.test.tsx`

### SET-settings-page

The system SHALL provide a Settings page, opened from the header Settings button or the Cmd/Ctrl+, shortcut, that groups preferences into named sections and applies each change immediately.

#### Scenario: Open the Settings page

- GIVEN the desktop is running
- WHEN the user clicks the header Settings button or presses Cmd/Ctrl+,
- THEN the main area shows the Settings page with its section navigation and preference controls

Evidence: `apps/desktop/src/App.tsx`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/App.test.tsx`, `apps/desktop/src/features/settings/SettingsPage.test.tsx`

## Flow

The header gear button and Cmd/Ctrl+, call `setActiveView('settings')`; the UI store switches the main area through the same unsaved-draft guard as other view changes, and terminal workspaces stay mounted behind it. A left rail lists the sections (Editor, Appearance, ACP agents); selecting one shows that section's card. The Editor section's Vim mode switch reuses `useEditorVim`, so toggling reconfigures any open CodeMirror editor at once (see [Desktop Editor](../architecture/editor.md#edt-vim-save)). The Appearance section's Theme picker lists the bundled palettes and a System option that follows the OS appearance, all through `useTheme` (see [Desktop Theme](desktop-theme.md#thm-dark-palette)). The ACP agents section lists the five preset agents (Devin, OpenCode, Claude Code, Codex, Gemini). Each row shows a status dot and word (`disconnected`, `connecting`, `connected`, or `error` with its message), an Enabled switch, a Connect or Disconnect button, a command-line field, a commit-prompt textarea, and, when the row differs from its preset, a Reset to default button. Command and prompt edits are drafts saved on blur only when changed; every save sends the full list of non-default rows to `acp_agents_set`, which validates all rows and stores none if any is invalid. Status is polled every 3 seconds only while the section is shown. The agents are used by the Editor's commit box (see [Desktop Editor](../architecture/editor.md#edt-acp-commit-message)).

## State changes

The Vim mode switch reads and writes `editor.vim` through the settings bridge. The Theme picker reads and writes `goko.theme` in local storage and applies `data-theme` on the document root. The ACP agents section reads and writes the settings key `acp.agents`, a JSON array of `{id, enabled, command, prompt}` holding only the rows that differ from their preset; missing, unreadable, or invalid JSON means all presets. The Editor's commit box stores its picked agent under `commit.agent`. The active Settings section is component state only; `activeView: 'settings'` lives in the in-memory UI store and returns to `terminal` when a session opens, a workspace is activated (even the active one), or the header Settings button is pressed again.

## Side effects

Writing Vim mode persists `editor.vim` via `settings_set`; writing the theme updates local storage and the root `data-theme` attribute. Connect spawns the agent's command and Disconnect kills it, and saving an agent that is enabled and connected with a changed command or a new disabled state disconnects it first. No other side effects.

## Authorization & constraints

Settings are global to the app, not scoped to a project or checkout, and the page renders regardless of workspace selection.

An ACP command line is split with shell-style quoting and no shell: leading `NAME=value` words become environment variables, the next word is the program, and the rest are arguments. It is rejected when empty, when a quote is unbalanced, or when it has no program word. A commit prompt over 4 KiB is rejected, and an empty prompt means the preset's default. An agent counts as available when its program word resolves on `PATH` (or, with a `/`, is an executable file); Connect is disabled for a disabled or unavailable agent. The preset commands are `DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max`, `opencode acp`, `npx -y @zed-industries/claude-agent-acp`, `npx -y @zed-industries/codex-acp`, and `gemini --experimental-acp`.

## Error paths

A failed `editor.vim` read leaves the switch off; a failed write leaves the previous value. A rejected ACP save shows the backend's message (naming the agent and the reason) under that row, keeps the typed text, and stores nothing; a failed Connect leaves the row in `error` with the reason, and an agent that exits later shows `error` too. The page defines no other error UI.

## Tests

`apps/desktop/src/features/settings/SettingsPage.test.tsx` covers the Vim switch read/write, theme read/apply/persist, and section switching. `apps/desktop/src/App.test.tsx` covers opening the page from the gear button and Cmd/Ctrl+,. The `SettingsPage` tests' ACP cases cover the five rows, enabling and editing with only non-default rows saved, the inline error on a rejected command, reset to default, connect and disconnect, error status, and disabled or unavailable agents; `acp` Rust tests cover preset merging, command parsing and validation, and connect/disconnect status.

## Related

- [Architecture overview](../architecture/overview.md)
- [Desktop Editor](../architecture/editor.md)
- [Desktop Theme](desktop-theme.md)

## Sources

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/App.test.tsx`
- `apps/desktop/src/components/use-theme.ts`
- `apps/desktop/src/components/ui/switch.tsx`
- `apps/desktop/src/features/settings/AcpAgentsSection.tsx`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/features/settings/SettingsPage.test.tsx`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/src-tauri/src/acp.rs`

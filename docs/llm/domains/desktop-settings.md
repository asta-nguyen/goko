# Desktop Settings

## Requirements

### SET-accounts

The system SHALL let the user keep several Claude Code and Codex accounts in a Settings section, sign each in through the agent's own login command, and choose one active account per agent, so that sessions started afterwards run with the active account's login while running sessions keep theirs.

#### Scenario: Add an account and switch to it

- GIVEN the Accounts section is open and only the Default Claude account exists
- WHEN the user adds an account named Work with a valid folder
- THEN Work is the active Claude account, the header Claude chip names Work, and the next Claude session starts with that account's folder

#### Scenario: Sign in after adding

- GIVEN the Accounts section is open
- WHEN the user adds an account
- THEN a terminal session titled `Sign in · <name>` opens and runs the agent's login command for that account in the account's folder

#### Scenario: Running sessions keep their account

- GIVEN a Claude session is running under the Default account
- WHEN the user sets another account active
- THEN the running session keeps its account and its row still names Default

#### Scenario: Reject an invalid account

- GIVEN the Accounts section is open
- WHEN the user adds an account whose name or folder duplicates an existing one
- THEN an inline error is shown and no account is stored

Evidence: `apps/desktop/src/features/settings/AccountsSection.tsx`, `apps/desktop/src-tauri/src/accounts.rs`, `apps/desktop/src-tauri/src/agents.rs`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src/features/accounts/queries.ts`, `apps/desktop/src/features/usage/UsageChips.tsx`, `apps/desktop/src/features/settings/AccountsSection.test.tsx`

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

The header gear button and Cmd/Ctrl+, call `setActiveView('settings')`; the UI store switches the main area through the same unsaved-draft guard as other view changes, and terminal workspaces stay mounted behind it. A left rail lists the sections (Editor, Appearance, Accounts, ACP agents); selecting one shows that section's card. The Editor section's Vim mode switch reuses `useEditorVim`, so toggling reconfigures any open CodeMirror editor at once (see [Desktop Editor](../architecture/editor.md#edt-vim-save)). The Appearance section's Theme picker lists the bundled palettes and a System option that follows the OS appearance, all through `useTheme` (see [Desktop Theme](desktop-theme.md#thm-dark-palette)). The ACP agents section lists the five preset agents (Devin, OpenCode, Claude Code, Codex, Gemini). Each row shows a status dot and word (`disconnected`, `connecting`, `connected`, or `error` with its message), an Enabled switch, a Connect or Disconnect button, a command-line field, a commit-prompt textarea, and, when the row differs from its preset, a Reset to default button. Command and prompt edits are drafts saved on blur only when changed; every save sends the full list of non-default rows to `acp_agents_set`, which validates all rows and stores none if any is invalid. Status is polled every 3 seconds only while the section is shown. The agents are used by the Editor's commit box (see [Desktop Editor](../architecture/editor.md#edt-acp-commit-message)).

The Accounts section shows a Claude Code card and a Codex card. Each lists the Default account first (read-only: its folder and usage, no Rename or Remove) and then the added accounts, each with its name, its folder in mono type, a usage line (limits and data age, or `no usage yet`), an Active marker or a **Set active** button, and a ⋯ menu with **Rename** (inline) and **Remove**. **Add account** opens an inline Name and Folder form; the folder follows `~/.claude-<slug>` or `~/.codex-<slug>` (the lowercase name with non-alphanumerics replaced by `-`) until the user edits it. Add, rename, remove, and set active call `accounts_add`, `accounts_rename`, `accounts_remove`, and `accounts_set_active`; each returns the whole account list, which replaces the cached list and refreshes the usage snapshot. After a successful add, and from each added row's **Sign in** button, the card starts the agent's login: `accounts_sign_in` returns the login command, and the section creates a session from it (the account folder as `cwd`, 80 by 24) and opens it in the Unassigned workspace, which switches the app to the terminal view; the Default row has no Sign in button. Row usage comes from the `accounts` map of `usage_get`. How an account affects launches, folders, and usage is in [Agent accounts](agent-accounts.md); the header usage chip's switch menu has a **Manage accounts…** item that calls the UI store's `openSettings('accounts')`.

## State changes

The Vim mode switch reads and writes `editor.vim` through the settings bridge. The Theme picker reads and writes `goko.theme` in local storage and applies `data-theme` on the document root. The ACP agents section reads and writes the settings key `acp.agents`, a JSON array of `{id, enabled, command, prompt}` holding only the rows that differ from their preset; missing, unreadable, or invalid JSON means all presets. The Editor's commit box stores its picked agent under `commit.agent`. The Accounts section reads and writes the settings key `accounts` through the `accounts_*` commands (shape in [Agent accounts](agent-accounts.md#state-changes)). The active Settings section is component state; the in-memory UI store also holds a one-shot `settingsSection`, set together with `activeView: 'settings'` by `openSettings(section)` (through the unsaved-draft guard), which the Settings page selects on arrival or while open and then clears, so opening Settings with the header button still starts on Editor. `activeView: 'settings'` returns to `terminal` when a session opens, a workspace is activated (even the active one), or the header Settings button is pressed again.

## Side effects

Writing Vim mode persists `editor.vim` via `settings_set`; writing the theme updates local storage and the root `data-theme` attribute. Connect spawns the agent's command and Disconnect kills it, and saving an agent that is enabled and connected with a changed command or a new disabled state disconnects it first. Adding an account creates its folder and symlinks shared entries into it, and add, remove, and set active wake the usage poller (see [Agent accounts](agent-accounts.md#acc-account-folders)); adding an account and pressing Sign in each start one login terminal session, and no account command stops or restarts a session. No other side effects.

## Authorization & constraints

Settings are global to the app, not scoped to a project or checkout, and the page renders regardless of workspace selection.

An ACP command line is split with shell-style quoting and no shell: leading `NAME=value` words become environment variables, the next word is the program, and the rest are arguments. It is rejected when empty, when a quote is unbalanced, or when it has no program word. A commit prompt over 4 KiB is rejected, and an empty prompt means the preset's default. An agent counts as available when its program word resolves on `PATH` (or, with a `/`, is an executable file); Connect is disabled for a disabled or unavailable agent. The preset commands are `DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max`, `opencode acp`, `npx -y @zed-industries/claude-agent-acp`, `npx -y @zed-industries/codex-acp`, and `gemini --experimental-acp`.

An account name is trimmed and must be 1 to 40 characters, unique per agent ignoring case, and never `Default`; its folder, after expanding a leading `~/` from `HOME`, must be absolute, free of NUL and newline characters, not the agent's Default folder, and not used by another account of that agent. The Default account cannot be renamed or removed, and removing an account never deletes its folder.

## Error paths

A failed `editor.vim` read leaves the switch off; a failed write leaves the previous value. A rejected ACP save shows the backend's message (naming the agent and the reason) under that row, keeps the typed text, and stores nothing; a failed Connect leaves the row in `error` with the reason, and an agent that exits later shows `error` too. A rejected account add shows the backend's message under the form, keeps the typed text and the form open, and stores nothing; a rejected rename shows its message under that row, which stays in edit mode. A failed `accounts_get` replaces a card's rows with the message, a failed Set active or Remove shows nothing and leaves the list unchanged, and a rejected sign-in (agent not on `PATH`, folder missing, daemon error) shows its message as an alert in that agent's card and leaves the account added. The page defines no other error UI.

## Tests

`apps/desktop/src/features/settings/SettingsPage.test.tsx` covers the Vim switch read/write, theme read/apply/persist, and section switching. `apps/desktop/src/App.test.tsx` covers opening the page from the gear button and Cmd/Ctrl+,. The `SettingsPage` tests' ACP cases cover the five rows, enabling and editing with only non-default rows saved, the inline error on a rejected command, reset to default, connect and disconnect, error status, and disabled or unavailable agents; `acp` Rust tests cover preset merging, command parsing and validation, and connect/disconnect status. `apps/desktop/src/features/settings/AccountsSection.test.tsx` covers the Default rows, add (trimmed values, inline rejection, folder following the name), the sign-in session started after an add and from the Sign in button, a rejected sign-in, set active, rename, remove, the usage line, and the hint; `apps/desktop/src/features/settings/SettingsPage.test.tsx` covers the Accounts nav entry and `openSettings('accounts')` selecting it on arrival and while open; the `accounts.rs` Rust tests cover validation, persistence, and the commands' rules.

## Related

- [Architecture overview](../architecture/overview.md)
- [Agent accounts](agent-accounts.md)
- [Desktop Editor](../architecture/editor.md)
- [Desktop Theme](desktop-theme.md)

## Sources

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/App.test.tsx`
- `apps/desktop/src/components/use-theme.ts`
- `apps/desktop/src/components/ui/switch.tsx`
- `apps/desktop/src/features/accounts/queries.ts`
- `apps/desktop/src/features/settings/AccountsSection.tsx`
- `apps/desktop/src/features/settings/AccountsSection.test.tsx`
- `apps/desktop/src/features/settings/AcpAgentsSection.tsx`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/features/settings/SettingsPage.test.tsx`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/features/usage/UsageChips.tsx`
- `apps/desktop/src/services/accounts.ts`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/src-tauri/src/accounts.rs`
- `apps/desktop/src-tauri/src/acp.rs`
- `apps/desktop/src-tauri/src/agents.rs`
- `apps/desktop/src-tauri/src/commands.rs`

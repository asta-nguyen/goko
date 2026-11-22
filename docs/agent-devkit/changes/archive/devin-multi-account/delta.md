# Delta

Merged into docs/llm/: 2026-11-22

## ADDED Requirements

### RNC-devin-accounts

Page: `docs/llm/domains/rune-chrome.md`
Search: checked the Rune chrome requirements, the only page with the RNC prefix. No requirement covers creating or signing in to a named Devin account with an isolated data root.

Requirement:

````md
### RNC-devin-accounts

On macOS and Linux, the system SHALL add a Devin account list alongside the Claude and Codex lists in Settings > Accounts, with an implicit Default and named accounts stored by name and dedicated data-root folder; named sessions and sign-in use that folder's isolated Devin data and configuration, while Default keeps the CLI's ambient login.

#### Scenario: add or sign in

- GIVEN the Devin binary is available
- WHEN a dedicated folder is registered or Sign in is pressed on its named row
- THEN a Devin sign-in terminal opens with `XDG_DATA_HOME` set to that folder, `XDG_CONFIG_HOME` set to its `config` subfolder, and no inherited `WINDSURF_API_KEY`

#### Scenario: unsafe or missing folder

- GIVEN a Devin profile folder is missing at launch, or the proposed or selected profile aliases another login location
- WHEN Goko validates the add, sign-in, or launch
- THEN Goko reports an error rather than falling through to another account

#### Scenario: remove the active account

- GIVEN a named Devin account is active
- WHEN its registration is removed
- THEN Default becomes active for future launches and the removed profile's files remain untouched

#### Scenario: Windows

- GIVEN Goko runs on Windows
- WHEN a user opens Settings > Accounts or starts the Devin preset
- THEN no named Devin controls are offered and the preset continues to use the ambient login

Evidence: `crates/app-core/src/accounts.rs`, `crates/app-core/src/agents.rs`, `apps/rune/src/settings.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/main.rs`
````

## MODIFIED Requirements

### RNC-session-account-label

Previously: only Claude and Codex sessions have launch-account labels.

Baseline:

````md
### RNC-session-account-label

The system SHALL label every Claude/Codex session row with the account it
runs under, derived from the session's own launch argv
(`account_name_of`): a `/usr/bin/env CLAUDE_CONFIG_DIR=…`/`CODEX_HOME=…`
word names the account whose folder matches (or the folder's basename when
the account was removed), and plain argv names the Default account —
independent of any later active-account switch.

#### Scenario: named-account session

- GIVEN a session whose argv starts `env CLAUDE_CONFIG_DIR=/accts/work`
- WHEN the sidebar renders its row
- THEN the label reads `Claude · work`

#### Scenario: removed-account session

- GIVEN a session whose argv names a folder no stored account owns
- WHEN the sidebar renders its row
- THEN the label shows the folder's last component

Evidence: `apps/rune/src/data.rs`, `apps/rune/src/sidebar.rs`
````

Replacement:

````md
### RNC-session-account-label

The system SHALL label every Claude/Codex session row and, on macOS and Linux, every Devin preset session row with the launch account, derived from the row's own argv: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, or `XDG_DATA_HOME` names the matching account (or the folder basename if removed), and plain argv names Default.

#### Scenario: named-account session

- GIVEN a session whose argv starts `env CLAUDE_CONFIG_DIR=/accts/work`
- WHEN the sidebar renders its row
- THEN the label reads `Claude · work`

#### Scenario: Devin account changed after launch

- GIVEN a Devin session launched with the data root for the named account `Work`
- WHEN another Devin account becomes active
- THEN the existing session still shows `Work`

#### Scenario: removed-account session

- GIVEN a session whose argv names a folder no stored account owns
- WHEN the sidebar renders its row
- THEN the label shows the folder's last component

Evidence: `apps/rune/src/data.rs`, `apps/rune/src/sidebar.rs`
````

### RNC-sidebar-row-actions

Previously: Devin always launches as Default.

Baseline:

````md
### RNC-sidebar-row-actions

The system SHALL reveal per-row hover actions matching the desktop: project
and worktree rows offer a New-session menu (agent presets discovered through
`app_core::agents`, plus a custom command) that issues `session.create` in
that checkout and records session metadata, and a "more actions" menu
(Open Editor, Refresh worktrees, Create worktree, Remove worktree on
worktree rows; Open Editor, Create worktree, Refresh worktrees, Remove
project on project rows) routed through the editor backend's guarded lanes;
session rows offer Stop and Close pane mapped to `session.stop` /
`session.delete`. A custom command is split into shell words — a quoted
argument stays one word — and an empty or unparseable command is refused
with a notice instead of starting a session.

#### Scenario: spawn a session from a project row

- GIVEN a registered project row with its hover actions revealed
- WHEN New session → an agent is chosen
- THEN `session.create` runs with the agent's resolved argv in the project
  checkout, metadata is recorded, and the session appears nested under it

#### Scenario: a preset without accounts launches

- GIVEN the Shell, OpenCode or Devin preset is chosen
- WHEN the worker resolves its argv
- THEN it launches as the Default account — only Claude and Codex resolve a
  stored account, so no preset is refused as an unknown agent

#### Scenario: two launches at once

- GIVEN two rows ask for an agent before either answer arrives
- WHEN the answers land, in either order
- THEN each answer starts its own request's session in its own checkout

#### Scenario: a refused launch

- GIVEN the account folder was removed or the agent's binary disappeared
- WHEN the argv reply fails
- THEN the failure is shown as the notice and no session starts

#### Scenario: remove a worktree

- GIVEN a worktree row's more-actions menu
- WHEN Remove worktree is chosen and confirmed
- THEN the worktree is removed through git, the list refreshes, and the
  editor's stale-checkout handling applies if it was open

#### Scenario: actions hidden until hover

- GIVEN the sidebar renders
- WHEN no row is hovered
- THEN no action menu is visible; hovering a row reveals its actions without
  shifting layout

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/agents.rs`, `crates/app-core/src/worktrees.rs`, `crates/app-core/src/projects.rs`
````

Replacement:

````md
### RNC-sidebar-row-actions

The system SHALL reveal per-row hover actions matching the desktop: project
and worktree rows offer a New-session menu (agent presets discovered through
`app_core::agents`, plus a custom command) that issues `session.create` in
that checkout and records session metadata, and a "more actions" menu
(Open Editor, Refresh worktrees, Create worktree, Remove worktree on
worktree rows; Open Editor, Create worktree, Refresh worktrees, Remove
project on project rows) routed through the editor backend's guarded lanes;
session rows offer Stop and Close pane mapped to `session.stop` /
`session.delete`. A custom command is split into shell words — a quoted
argument stays one word — and an empty or unparseable command is refused
with a notice instead of starting a session.

#### Scenario: spawn a session from a project row

- GIVEN a registered project row with its hover actions revealed
- WHEN New session → an agent is chosen
- THEN `session.create` runs with the agent's resolved argv in the project
  checkout, metadata is recorded, and the session appears nested under it

#### Scenario: a preset without accounts launches

- GIVEN the Shell or OpenCode preset is chosen
- WHEN the worker resolves its argv
- THEN it launches with its existing command and no stored account lookup

#### Scenario: named Devin account launches

- GIVEN a named Devin account is active on macOS or Linux
- WHEN the Devin preset is chosen
- THEN the new session starts in that checkout using the active account's isolated environment

#### Scenario: two launches at once

- GIVEN two rows ask for an agent before either answer arrives
- WHEN the answers land, in either order
- THEN each answer starts its own request's session in its own checkout

#### Scenario: a refused launch

- GIVEN the account folder was removed or the agent's binary disappeared
- WHEN the argv reply fails
- THEN the failure is shown as the notice and no session starts

#### Scenario: remove a worktree

- GIVEN a worktree row's more-actions menu
- WHEN Remove worktree is chosen and confirmed
- THEN the worktree is removed through git, the list refreshes, and the
  editor's stale-checkout handling applies if it was open

#### Scenario: actions hidden until hover

- GIVEN the sidebar renders
- WHEN no row is hovered
- THEN no action menu is visible; hovering a row reveals its actions without
  shifting layout

Evidence: `apps/rune/src/sidebar.rs`, `apps/rune/src/daemon.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/agents.rs`, `crates/app-core/src/worktrees.rs`, `crates/app-core/src/projects.rs`
````

### RNC-usage-accounts

Previously: the account selector only populated rows for Claude and Codex.

Baseline:

````md
### RNC-usage-accounts

The system SHALL read the usage snapshot and the accounts view on the
editor backend worker roughly every five seconds (`Req::Usage` carrying the
previous snapshot, plus an idle `wake_after` so a quiet UI still refreshes),
store them in `Data::usage`/`Data::accounts` only on successful replies,
and render one header chip per provider that has any limit or at least two
accounts: each present window shows its `windowLabel` and `used_pct` —
muted "reset" once `resets_at` has passed, the rounded percentage in
muted/warning/danger tones otherwise — and the chip tooltip lists each
window's reset and "Updated <age|unknown>". With two or more accounts the
chip suffixes the active account's name and opens a menu listing the
accounts (checked on the active one, each with its own usage summary in the
same expiry-aware form as the chips — a window that already reset reads
"reset", not its last percentage) whose selects send `AccountSetActive`,
plus "Manage accounts…" which opens the Settings page's Accounts section.
Account mutations (`AccountAdd`, `AccountRename`, `AccountRemove`,
`AccountSetActive`) run on the worker, mutate the cached view only on success
and refresh the usage read; errors surface as the app notice.

#### Scenario: quota chips render and refresh

- GIVEN a usage snapshot with a `five_hour` limit at 42 %
- WHEN the view renders
- THEN the Claude chip shows `Claude 5h 42%` and a ~5 s later worker read
  refreshes it

#### Scenario: elapsed window

- GIVEN a limit whose `resets_at` is in the past
- WHEN the view renders
- THEN the window shows a muted "reset" instead of a percentage

#### Scenario: multiple accounts

- GIVEN the Claude accounts view holds two accounts
- WHEN the active account differs from the chip's current name
- THEN the chip shows `Claude · <active name>` and its menu can activate
  the other account or open Settings → Accounts

#### Scenario: failed mutation

- GIVEN an account remove whose worker job fails
- WHEN the reply arrives
- THEN the cached view is unchanged and the notice shows the error

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/data.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/usage.rs`, `crates/app-core/src/accounts.rs`
````

Replacement:

````md
### RNC-usage-accounts

The system SHALL refresh the usage snapshot and account list through the editor worker roughly every five seconds and check the active account in the Agents dropdown's account selectors. Claude and Codex retain their existing selectors and expiry-aware usage summaries. On macOS and Linux the Devin selector appears whenever at least one named Devin account exists, even without quota data, and never attributes Default's meter to a named account. "Manage accounts…" opens Settings > Accounts. Account mutations run on the worker, update the account view only on success, and report failures through the existing notice.

#### Scenario: quota windows refresh

- GIVEN a usage snapshot with a Claude five-hour limit
- WHEN the Agents dropdown renders and the limit expires
- THEN the meter refreshes and shows "reset" rather than the old percentage

#### Scenario: multiple Claude accounts

- GIVEN the Claude accounts view holds two accounts and a usage record is present
- WHEN the Agents dropdown opens
- THEN it checks the active account, and the user can select the other account or open Settings > Accounts

#### Scenario: multiple Devin accounts with no usage

- GIVEN the named Devin account `Work` and Default are available
- WHEN the user selects `Work` in the Agents dropdown
- THEN `Work` is checked on success and no quota is shown for it

#### Scenario: failed mutation

- GIVEN an account remove whose worker job fails
- WHEN the reply arrives
- THEN the cached view is unchanged and the notice shows the error

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/data.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/usage.rs`, `crates/app-core/src/accounts.rs`
````

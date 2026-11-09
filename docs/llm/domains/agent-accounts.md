# Agent accounts

## Requirements

### ACC-account-folders

The system SHALL create an added account's folder with mode 0700 when it is missing and symlink into it the entries its agent can share from the Default folder, never replacing an entry that already exists and never linking credentials, `.claude.json`, or Codex `sessions`.

#### Scenario: Shared entries are linked

- GIVEN the Default Claude folder holds `projects`, `settings.json`, and `.credentials.json`
- WHEN an account is added with a folder that does not exist yet
- THEN the folder is created with mode 0700, `projects` and `settings.json` in it are symlinks to the Default folder's entries, and `.credentials.json` is not linked

#### Scenario: An existing entry is kept

- GIVEN the chosen folder already holds its own `settings.json`
- WHEN the account is added
- THEN that file is left as it was and is not replaced by a link

Evidence: `apps/desktop/src-tauri/src/accounts.rs`

### ACC-account-switcher

The system SHALL, for an agent with two or more accounts, name the active account in the header usage chip, let the user switch the active account from the chip's menu, and show the account name in the agent's new-session menu item and on each of its session rows.

#### Scenario: Switch from the chip

- GIVEN two Claude accounts exist and Work is active
- WHEN the user opens the Claude chip and chooses Default
- THEN Default becomes the active Claude account and the chip names it

#### Scenario: One account looks as before

- GIVEN an agent has only its Default account
- WHEN the header, the new-session menu, and the session rows render
- THEN no account name is shown and the chip opens no menu

Evidence: `apps/desktop/src/features/usage/UsageChips.tsx`, `apps/desktop/src/features/sessions/SessionLaunch.tsx`, `apps/desktop/src/features/sessions/SessionItems.tsx`, `apps/desktop/src/services/accounts.ts`, `apps/desktop/src/features/usage/UsageChips.test.tsx`, `apps/desktop/src/features/sessions/SessionLaunch.test.tsx`, `apps/desktop/src/features/sessions/SessionItems.test.tsx`

### ACC-launch-account

The system SHALL start a Claude or Codex session under the agent's active account by running the agent through `/usr/bin/env` with `CLAUDE_CONFIG_DIR` or `CODEX_HOME` set to that account's folder, launching the Default account exactly as before and rejecting the launch when a named account's folder no longer exists.

#### Scenario: A named Claude account

- GIVEN the active Claude account is Work with folder `/f`
- WHEN a Claude session is created
- THEN its argv is `/usr/bin/env CLAUDE_CONFIG_DIR=/f <claude> --session-id <uuid> --settings <json>`

#### Scenario: The Default account is unchanged

- GIVEN the Default Codex account is active
- WHEN a Codex session is created
- THEN its argv is the single resolved `codex` path

#### Scenario: A missing folder

- GIVEN the active account's folder has been deleted
- WHEN a session of that agent is created
- THEN the launch is rejected with `invalid_argument` and the hint "restore the folder or pick another account in Settings › Accounts"

Evidence: `apps/desktop/src-tauri/src/agents.rs`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/accounts.rs`, `apps/desktop/src-tauri/src/commands/tests.rs`

### ACC-usage-per-account

The system SHALL read usage separately for every account of an agent, so that the usage snapshot reports the active account's limits as the agent's limits and every account's last known limits in an `accounts` map keyed by agent and account id.

#### Scenario: Two Claude accounts

- GIVEN two Claude accounts whose statusline captures are in different folders and Work is active
- WHEN the usage snapshot is read
- THEN `accounts["claude:<id>"]` holds each account's limits and `claude` equals Work's

#### Scenario: Switching the active account

- GIVEN a snapshot with two accounts
- WHEN the other account becomes active
- THEN `claude` changes to its limits and the `accounts` map is unchanged

Evidence: `apps/desktop/src-tauri/src/usage.rs`

## Flow

Accounts are stored per agent (`claude`, `codex`) as `{id, name, dir}` rows; each agent also has an implicit Default account (id `default`, never stored) whose folder is `$CLAUDE_CONFIG_DIR` or `~/.claude` for Claude and `$CODEX_HOME` or `~/.codex` for Codex. `accounts.<agent>.active` names the account new sessions use. Choosing accounts in Settings is in [Desktop Settings](desktop-settings.md#set-accounts).

**Launch.** `session_create` for agent `claude` or `codex` resolves the agent's active account (`accounts::resolve`) and passes it to `agents::argv_for`. The Default account launches as before. A named Claude account prepends `/usr/bin/env CLAUDE_CONFIG_DIR=<dir>`; the user's original `statusLine.command` is read from `<dir>/settings.json`, and the injected statusline capture file is `$GOKO_HOME/usage/claude/<account-id>/<session-id>.json` (Default keeps the flat `usage/claude/<session-id>.json`; the activity file path is the same for every account). A named Codex account prepends `/usr/bin/env CODEX_HOME=<dir>`. The folder is one argv element, so spaces need no quoting. Other agents never get env words. The Codex activity matcher accepts `env`, then `NAME=value` words, then a binary with stem `codex`, so Codex activity keeps working under a named account.

**Sign in.** For an added account, `accounts_sign_in` returns `{command, cwd, title}`: the agent's own login command (`env CLAUDE_CONFIG_DIR=<dir> <claude> auth login` or `env CODEX_HOME=<dir> <codex> login`, the binary resolved on `PATH`) with each word shell-quoted by `shlex::try_join`, the account folder as `cwd`, and the title `Sign in · <name>`. The Settings UI creates a session from it through the existing custom-command path (`shlex::split` restores the words) and opens it in the Unassigned workspace; the CLI itself opens the browser and stores the login in its own place, so Goko never sees a credential. It is requested after an account is added and from each added row's Sign in button; the Default account has none. When the command exits the session stays listed as exited until deleted.

**Usage.** The usage poller reads the stored accounts each tick (and when woken) and reads each account's source: for Claude, the newest capture file reporting `rate_limits` in that account's capture folder; for Codex, the newest rollout under `<dir>/sessions` (the Default account uses the resolved `CODEX_HOME`). `claude` and `codex` in the snapshot describe the active account; `claude.sessions` (context fill by session id) is merged across every Claude capture folder. Account commands that add, remove, or activate an account wake the poller so the snapshot changes at once; rename does not.

**Switcher.** With two or more accounts for an agent, the header chip reads `<Agent> · <active name>` (and stays visible before any usage arrives) and opens a menu: one radio row per account with its usage line, and **Manage accounts…**, which opens Settings on Accounts. Picking a row sets that account active. The agent's item in the new-session menu shows `· <active name>`, and its session rows show the account derived from the session's own argv, so a row keeps naming the account it started with after the active account changes. With one account none of this appears and the chip is a plain badge.

## State changes

The settings key `accounts` holds `{"claude":{"active":"<id>","items":[{"id","name","dir"}]},"codex":{…}}`; a missing or malformed value, an unknown or removed active id, or an unreadable store reads as Default only with Default active. Ids are the first 8 hex characters of a v4 UUID. The usage snapshot gains `accounts`: one entry per account, keyed `"<agent>:<account-id>"`, each `{limits, updated_at}` with only the limits that exist in window order (empty and `null` for an account with no data yet). Account folders and their symlinks live wherever the user chose; capture folders live under `$GOKO_HOME/usage/claude/<account-id>/`.

## Side effects

Adding an account creates its folder when missing (mode 0700, parents included) and symlinks shared entries into it. Shared entries for Claude are `projects`, `settings.json`, `CLAUDE.md`, `skills`, `commands`, `agents`, `plugins`, and `hooks`; for Codex `config.toml`, `AGENTS.md`, `skills`, and `prompts`. A link that cannot be made is skipped and the add still succeeds. Removing an account deletes only its stored row, never its folder or capture files. At startup the capture files older than seven days are pruned in the Default capture folder and in every named account's.

## Authorization & constraints

Goko never reads credentials: the login for a folder stays in the agent's own storage (a Keychain entry or `auth.json`), and credential files are never linked between folders. Usage is read passively from capture files and rollouts; an account that has not run a session recently shows its last known value and age, and usage consumed outside Goko is not seen. Account names and folders are validated as described in [Desktop Settings](desktop-settings.md#authorization--constraints). Only Claude and Codex have accounts; an unknown agent name in an account command is `invalid_argument`.

## Error paths

A named account whose folder is missing fails the launch with `invalid_argument` rather than letting the agent recreate an empty folder. `accounts_sign_in` rejects, also with `invalid_argument`, the Default account, an unknown account or agent, a missing folder, and an agent binary not found on `PATH` (hint: install it). A store that cannot be read makes launches use the Default account and the poller read Default only. An unreadable capture file keeps that account's previous limits. A capture folder owned by no account is never read.

## Tests

`apps/desktop/src-tauri/src/accounts.rs` (validation, persistence, folder creation and linking, resolve, the sign-in command and its rejections), `apps/desktop/src-tauri/src/agents.rs` (argv for Default and named accounts), `apps/desktop/src-tauri/src/commands/tests.rs` (launch resolution and the missing-folder error), `apps/desktop/src-tauri/src/activity.rs` (Codex recognition behind `env`), and `apps/desktop/src-tauri/src/usage.rs` (per-account snapshot, merged sessions, kept previous values, the refresh signal) hold the Rust tests. `apps/desktop/src/features/usage/UsageChips.test.tsx`, `apps/desktop/src/features/sessions/SessionLaunch.test.tsx`, and `apps/desktop/src/features/sessions/SessionItems.test.tsx` cover the switcher, the menu hint, and the row label; the Settings side is covered in [Desktop Settings](desktop-settings.md#tests).

## Related

- [Architecture overview](../architecture/overview.md)
- [Desktop Settings](desktop-settings.md)

## Sources

- `apps/desktop/src-tauri/src/accounts.rs`
- `apps/desktop/src-tauri/src/activity.rs`
- `apps/desktop/src-tauri/src/agents.rs`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/commands/tests.rs`
- `apps/desktop/src-tauri/src/usage.rs`
- `apps/desktop/src/features/accounts/AccountUsageSummary.tsx`
- `apps/desktop/src/features/accounts/queries.ts`
- `apps/desktop/src/features/sessions/SessionItems.tsx`
- `apps/desktop/src/features/sessions/SessionItems.test.tsx`
- `apps/desktop/src/features/sessions/SessionLaunch.tsx`
- `apps/desktop/src/features/sessions/SessionLaunch.test.tsx`
- `apps/desktop/src/features/usage/UsageChips.tsx`
- `apps/desktop/src/features/usage/UsageChips.test.tsx`
- `apps/desktop/src/services/accounts.ts`
- `apps/desktop/src/services/usage.ts`
- `apps/desktop/src/stores/ui.ts`

# Multiple Claude and Codex accounts

## Goal

A user with more than one Claude Code or Codex account can keep them side by side in Goko, see each
account's usage, and choose which account new sessions use, without logging out and back in.
Quota cannot be merged: limits are enforced per account by the provider, so this change only
makes switching fast and visible.

## Scope

In:
- Account profiles per agent (`claude`, `codex`), persisted in the settings table under `accounts`.
- An implicit **Default** account per agent that is today's behavior, unchanged.
- One **active** account per agent. New sessions of that agent use it; running sessions keep the
  account they started with.
- Launch injection: a non-default account starts the agent through `/usr/bin/env` with
  `CLAUDE_CONFIG_DIR=<dir>` (Claude) or `CODEX_HOME=<dir>` (Codex). The Default account launches
  exactly as today.
- Adding an account creates its folder and links the entries that Claude and Codex can share with
  the default folder (settings, skills, plugins, session history for Claude).
- Signing in: after an account is added, and from a **Sign in** button on each added account,
  Goko starts the agent's own login command under that account in a terminal session.
- Per-account usage, read passively from what Goko already reads (Claude statusline captures,
  Codex rollouts), one source per account folder.
- UI: the header usage chip becomes the account switcher, a new Settings section **Accounts**,
  the active account named in the "+" new-session menu, and the account named on session rows.
- Wiki: a `SET-accounts` requirement (see `delta.md`); `document-wiki` adds scoped coverage for the
  usage and launch behavior after the code checks, because no wiki page covers them today.

Out:
- Merging or pooling quota, automatic failover on a limit, and stopping or restarting a running
  session on the user's behalf. The user restarts a session themselves.
- Calling provider usage endpoints, reading tokens, auth files or the Keychain. Usage stays
  passive; an account that has not run recently shows its last known value and age.
- Resuming a Codex session under another account (rollouts live in each `CODEX_HOME`).
- Any change to `crates/protocol`, `crates/daemon`, or the `goko.db` schema.
- ACP adapters (commit message agents), Devin, OpenCode, and Shell sessions.
- Windows, Linux, and WSL specifics; Goko is a macOS desktop app.

## Behavior

**Accounts.** An account is `{id, name, dir}`. Each agent has the Default account (id `default`,
not stored; dir is `$CLAUDE_CONFIG_DIR` or `~/.claude` for Claude and `$CODEX_HOME` or `~/.codex`
for Codex, the resolution Goko already uses) plus any added accounts. `accounts.<agent>.active`
names the active one; a missing, unknown, or removed id means Default.

**Launch.** `session_create` for agent `claude` or `codex` uses that agent's active account.
- Default: argv is unchanged.
- Other account, Claude: `[/usr/bin/env, CLAUDE_CONFIG_DIR=<dir>, <claude>, --session-id, <id>,
  --settings, <json>]`. The user's original `statusLine.command` is read from
  `<dir>/settings.json`. The usage capture file is `usage/claude/<account-id>/<session-id>.json`
  (Default keeps `usage/claude/<session-id>.json`). The activity file path is unchanged.
- Other account, Codex: `[/usr/bin/env, CODEX_HOME=<dir>, <codex>]`.
- A non-default account whose folder no longer exists fails the launch with an
  `invalid_argument` error and a hint, instead of letting the agent recreate an empty folder and
  run the wrong account.

**Which account a session uses.** Derived from the session's argv: the `CLAUDE_CONFIG_DIR=` or
`CODEX_HOME=` word after `env` matched to an account's dir; no such word means Default; a word that
matches no account (a removed one) is labelled by its folder's last path component. Nothing is
stored per session.

**Switching.** Picking an account in the chip menu or in Settings sets `accounts.<agent>.active`.
It applies to sessions created afterwards, and the usage snapshot is refreshed at once so the chip
changes without waiting for the next poll tick.

**Add.** The user gives a name and a folder (default `~/.claude-<name>` or `~/.codex-<name>`, `~`
expanded by the backend). Goko creates the folder with mode 0700 if missing, then for each shared
entry that exists in the default folder and not in the new one creates a symlink to it. Shared
entries, Claude: `projects`, `settings.json`, `CLAUDE.md`, `skills`, `commands`, `agents`,
`plugins`, `hooks`. Codex: `config.toml`, `AGENTS.md`, `skills`, `prompts`. Never linked:
credential files (`auth.json`, `.credentials.json`), `.claude.json`, and Codex `sessions`
(sharing it would mix the accounts' usage). A link that cannot be made is skipped; the add still
succeeds. Linking `projects` is what lets `/resume` in one Claude account list the other's sessions.
The new account becomes the agent's active account.

**Sign in.** Goko never handles the login itself: it starts the agent's own login command under
the account, in a terminal session the user completes locally (the CLI opens the browser and
stores the token in its own place). The command is `env CLAUDE_CONFIG_DIR=<dir> claude auth login`
for Claude and `env CODEX_HOME=<dir> codex login` for Codex, run with the account's folder as the
working directory and titled `Sign in · <name>`. It starts automatically after a successful
**Add account**, and from a **Sign in** button on each added account row (to log in again). The
session opens in the Unassigned workspace and is shown; when the command exits the session stays
listed as exited until deleted. Goko does not detect or show login state, and the Default account
has no Sign in button (its login is the user's own).

**Rename / Remove.** Rename changes the name only. Remove deletes the account entry and never the
folder; running sessions are unaffected; if it was active, Default becomes active. Default can be
neither renamed nor removed.

**Usage.** Each poll tick reads the `accounts` setting and, for every account of each agent, reads
that account's source: Claude, the newest capture file reporting `rate_limits` in the account's
capture folder; Codex, the newest rollout under `<dir>/sessions`.
- `UsageSnapshot.claude` and `.codex` describe the **active** account, so existing readers keep
  working. `claude.sessions` (context fill by session id) is merged across all Claude capture
  folders.
- A new `accounts` map holds every account's non-empty limits (in window order) and update time,
  keyed `"<agent>:<account-id>"`.
- Values age exactly as today: the age is shown and a window past its reset reads `reset`.

**Header chip.** With only the Default account for an agent, the chip is exactly today's. With two
or more, the chip reads `<Agent> · <active name>` followed by today's limits, and clicking it opens
a menu: one row per account with the active one marked, its name, its limits and age (or "no usage
yet"); then **Manage accounts…**, which opens Settings › Accounts. The chip stays visible, without
limits, while an agent has two or more accounts even when no usage is known yet.

**Settings › Accounts.** Two cards, Claude Code and Codex. Each lists the Default row (read-only,
shows its folder) and the added accounts: name, folder in mono, limits and age, an Active marker or
**Set active**, a **Sign in** button (added accounts only), and a menu with **Rename** and
**Remove**. **Add account** opens an inline name and folder form with inline validation errors. A one-line hint says that
settings, skills and plugins are linked from the default folder and credentials are never shared.

**New-session menu and session rows.** With two or more accounts for an agent, its "+" menu item
shows `· <active name>` in muted text, and its session rows show the session's account name in
muted text. With one account nothing changes.

## Edge cases

- Name: 1 to 40 characters after trim, unique per agent, case-insensitive. Folder: absolute after
  `~` expansion, no NUL or newline, not the default folder, unique per agent. An invalid add or
  rename stores nothing and shows the reason inline.
- Malformed `accounts` JSON reads as "Default only", like `acp.agents`; a write replaces it.
- Two writers: each command reads, changes, and writes the whole `accounts` value in one call; a
  lost update between two simultaneous commands is accepted (single user, one window).
- The user exports their own `CLAUDE_CONFIG_DIR`/`CODEX_HOME` in the shell Goko starts from: that
  value is the Default folder and keeps being inherited; non-default accounts override it through
  the `env` words.
- `activity.rs` recognizes a Codex session by a lone argv whose stem is `codex`; it must also accept
  `[env, NAME=value…, <bin>]` whose bin stem is `codex`, or Codex activity would vanish for
  non-default accounts. Claude detection uses `--session-id` and is unaffected.
- A shared entry that Claude or Codex later replaces by rename-and-write (for example
  `settings.json`) becomes an independent copy; the account keeps working, it just stops following
  the default folder.
- A removed account's capture folder stays on disk; it is ignored once no account owns it, and
  `prune_old` removes old files as it does for Default.
- Usage of an account that was also used elsewhere (claude.ai, another machine) is not seen; the
  shown value can be optimistic until that account runs a session here (known limit of passive
  reading).
- Switching the active account never changes a running session, and the session row keeps showing
  its own account.
- Deleting a folder out from under an account fails the next launch with the error above; the
  other accounts are unaffected.
- Sign in with the agent's binary missing from `PATH`: the sign-in launch fails with
  `invalid_argument` and the usual "install it" hint; the account stays added. A missing account
  folder fails it like any launch.
- The login command exits non-zero or the user abandons the browser step: the session shows as
  exited, Goko stores nothing, and the user can press Sign in again.

## Interfaces

- Setting `accounts`: JSON `{"claude":{"active":"<id>","items":[{"id","name","dir"}]},"codex":{…}}`.
  Default is implicit and never stored.
- Backend module `accounts.rs` (app layer, `apps/desktop/src-tauri/`): read and validate the
  setting, add, rename, remove, set active, link shared entries, resolve a launch account.
- Tauri commands: `accounts_get`, `accounts_add(agent, name, dir)`,
  `accounts_rename(agent, id, name)`, `accounts_remove(agent, id)`,
  `accounts_set_active(agent, id)`. `accounts_get` returns each agent's accounts including Default
  with its resolved folder, and the active id. `session_create` is unchanged.
- Tauri command `accounts_sign_in(agent, id) -> {command, cwd, title}` for an added account: the
  login command above with the folder quoted for the custom-command path, the account folder as
  `cwd`, and the title. It rejects Default, an unknown id, a missing folder, and an agent binary
  not on `PATH` with `invalid_argument`. The UI starts it through the existing `session_create`
  custom-command arguments (`command` and `cwd`), so no new session API exists.
- `agents::argv_for` takes the resolved account (id and optional dir) and returns the argv above.
- `UsageSnapshot` (Rust `usage.rs`, TS `services/usage.ts`) gains `accounts`: a map of
  `{limits: Limit[], updated_at}`.
- Frontend: `services/accounts.ts` (including the helper that derives an account from a session
  argv), `features/accounts/queries.ts` (`useAccounts` and mutations, invalidating usage on
  change), `features/settings/AccountsSection.tsx` (new section id `accounts` in `SettingsPage`),
  `features/usage/UsageChips.tsx` (menu), `SessionLaunch.tsx` (menu hint),
  `features/sessions/SessionItems.tsx` (label). **Manage accounts…** opens Settings on the Accounts
  section through a one-shot `settingsSection` value in `stores/ui.ts` that `SettingsPage` reads
  on mount and clears; opening Settings otherwise still starts on Editor.
- On-disk: `usage/claude/<account-id>/` capture folders; account folders chosen by the user.

## Verification

Task 1 is a probe on the installed Claude and Codex with a throwaway second login. It must confirm:
a second `CLAUDE_CONFIG_DIR` keeps a separate login; its statusline JSON reports `rate_limits`;
with `projects` linked, `/resume` under the second account lists and opens the first account's
session; Claude runs normally with the shared entries linked; a second `CODEX_HOME` keeps a
separate login and writes rollouts under itself. A result that contradicts this design returns it
to approval.

- Rust unit tests: validation and persistence of `accounts` (invalid, duplicate, malformed JSON,
  removed active falls back to Default); `argv_for` unchanged for Default and env-prefixed for
  others, including a folder with a space; missing folder rejected at launch; `is_codex` with the
  `env` prefix; shared-entry linking in temp folders (skips existing and missing entries, never
  links credentials or Codex `sessions`); per-account usage reading with the active account in
  `claude`/`codex` and every account in `accounts`; account derived from argv; the sign-in command
  for Claude and Codex (folder with a space quoted, title, `cwd`) and its rejections.
- Vitest: Accounts section add, rename, remove, set active, validation errors; chip unchanged with
  one account and a menu with two; switching through the chip; "+" menu hint; session row label;
  a successful add starts and opens the sign-in session, the Sign in button appears on added rows
  only and does the same, and a rejected sign-in shows its message inline.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`,
  `pnpm -C apps/desktop typecheck`, and the Vitest suite.
- Manual on the built app with two real accounts: start a session under Default, switch the active
  account, start a second session and confirm each shows its own account and usage, confirm the
  first session keeps its account, confirm the login terminal opens right after Add and that
  finishing it signs the new account in, and confirm the Codex activity dot still works under a
  non-default account.

## Related context

- [Desktop Settings](../../../../llm/domains/desktop-settings.md)

## Previous work

- [Agent usage and limit indicators](../2026-11-04-agent-usage-limits/design.md)
- [Agent activity: working, needs you, done](../../2026-11-05-agent-activity/design.md)

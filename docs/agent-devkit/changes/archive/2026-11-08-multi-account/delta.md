# Delta

Merged into docs/llm/: 2026-11-08

## ADDED Requirements

### SET-accounts

Page: `docs/llm/domains/desktop-settings.md`
Search: checked every page with prefix SET (`docs/llm/domains/desktop-settings.md`: SET-acp-agents, SET-settings-page); no existing requirement covers agent accounts.

Requirement:

````md
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
````

# Delta

Merged into docs/llm/: 2026-11-22

## ADDED Requirements

### RNC-agent-handoff

Page: `docs/llm/domains/rune-chrome.md`
Search: checked all RNC requirements; none covers transferring work context between preset CLI agents or accounts.

Requirement:

````md
### RNC-agent-handoff

The system SHALL let the user hand off the focused pane of a registered project's Claude Code, Codex, Devin, OpenCode, or Antigravity preset tab to any installed preset agent and one of its supported accounts, opening a new tab in the source checkout without changing the original pane, its history, or the active account for future launches.

#### Scenario: local history available after quota runs out

- GIVEN the focused preset pane has an attributable local saved conversation, even if its account cannot answer
- WHEN the user chooses Hand off to, reviews the bounded text, and chooses Open & Copy Context
- THEN the requested destination starts a fresh session with its own account and the recorded checkout, the approved text is copied only after the new session is created, and no source model call, prompt submission, or terminal input occurs

#### Scenario: long conversation

- GIVEN the saved conversation does not fit in 16 KiB
- WHEN the handoff review opens
- THEN it keeps the first user message and the newest messages, says how many earlier messages were omitted, and disables opening while the user's edits exceed 16 KiB

#### Scenario: missing or unverifiable history

- GIVEN the exact provider conversation cannot be attributed or read, including an exited non-Claude pane started without a conversation ID
- WHEN the handoff review opens
- THEN it says there is no verified conversation text, offers an explicit conversation ID where the provider can verify it, and allows the user to proceed with checkout information and their own notes without including another chat

#### Scenario: explicit native Claude fork

- GIVEN the source and destination are Claude Code on macOS or Linux, the source transcript is verified, and the destination profile shares the source's Claude `projects` folder
- WHEN the user explicitly chooses to fork the full saved conversation instead of copying an excerpt
- THEN the new Claude tab uses the destination profile, original checkout, `--resume` with the source conversation ID, `--fork-session` and a fresh conversation ID, and nothing is copied

#### Scenario: fork unavailable

- GIVEN the destination Claude profile does not share the source's `projects` folder
- WHEN the user opens the destination picker
- THEN the fork option is not offered and the dialog says why, while the context handoff remains available

#### Scenario: cancellation, invalid target or failed launch

- GIVEN the user dismisses a pending review before launch, the source pane, checkout or selected destination is no longer valid, or the new session fails to start
- WHEN the worker or daemon finishes resolving the handoff
- THEN no new tab opens, nothing is copied, the review stays available after a failure, and an invalid target is explained without trying a different account

#### Scenario: account availability follows the preset

- GIVEN a destination that does not support named accounts in Goko, or a Windows host
- WHEN the user opens the destination picker
- THEN it offers only the destination's ambient Default and does not advertise unsupported account selection

Evidence: `apps/rune/src/handoff.rs`, `apps/rune/src/tab_debug.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/handoff.rs`, `crates/app-core/src/accounts.rs`, `crates/app-core/src/agents.rs`, `crates/app-core/src/session_debug.rs`
````

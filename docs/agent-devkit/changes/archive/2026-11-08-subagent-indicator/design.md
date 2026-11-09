# Sub-agent indicator on session rows

## Goal

When an agent session (for example Claude Code) starts another agent CLI such as Devin through its
shell tool, the session's sidebar row says so while that agent runs, so the user can tell the
session is busy delegating. Today the sub-agent is only a child process inside the session's own
terminal: Goko owns no session for it and shows nothing.

## Scope

In:
- An additive `pid` field on `SessionInfo` (protocol and daemon): the child's pid once spawned.
- A new app-layer module `subagents.rs` in `apps/desktop/src-tauri/` that, for every running
  session with a pid, finds descendant processes that are known agent CLIs, and publishes them
  (poller, `subagents_get` command, `subagents://updated` event), mirroring `activity.rs`.
- A muted indicator on the session row naming the running sub-agents.
- Wiki and `ARCHITECTURE.md` updates for the new field and indicator.

Out:
- Opening, attaching to, or showing the output of a sub-agent. Its output goes to its parent
  agent's pipe, not to a Goko terminal, and stays there.
- A real Goko session for a sub-agent, and any way for an agent to start one through Goko.
- Sub-agents that run inside the agent's own process (for example Claude Code's built-in Task
  tool): they are not processes, so they are not detected.
- Nested rows, history of finished sub-agents, killing a sub-agent from the UI, notifications.
- Any other protocol or daemon change, Windows, and Linux specifics (macOS only).

## Behavior

**Detection.** Every 2 seconds, if at least one running session has a pid, the backend reads the
process table once (`ps -axww -o pid=,ppid=,command=`, run with an argv and no shell) and, for each
running session, walks the descendants of its pid at any depth through `ppid`. A descendant is a
**sub-agent** when its program is one of `claude`, `codex`, `opencode`, or `devin`: the file stem of
the first word of its command, or, when that word is `node` or `bun`, of the first word after it.
The session's own process is never a sub-agent of itself. A wrapper such as `zsh -c … devin …` is
not matched, only the `devin` process under it.

**Publication.** The backend keeps a map from Goko session id to its sub-agents, each
`{agent, pid, command}` (the command line truncated to 200 characters). It sends the full map in
`subagents://updated` only when it changed; `subagents_get` returns the current map. A session that
is not `running`, has no pid, or has no sub-agent has no entry.

**Indicator.** A session row with sub-agents shows `↳ devin`; the same agent twice shows
`↳ devin ×2`; different agents are joined with a comma in alphabetical order (`↳ codex, devin ×2`).
The text is muted and small, truncates before the context percentage, and has a title listing each
sub-agent as `<agent> (pid <pid>): <command>`. Its accessible name reads `Running sub-agents:`
followed by the same list. A row without sub-agents is unchanged.

## Edge cases

- A sub-agent that runs for less than the 2 second tick can be missed; the indicator is a
  best-effort signal, not a log.
- A session whose daemon is older than the app sends no `pid`: it gets no indicator until the
  daemon restarts with the new binary.
- A first word with a space in its path (an install directory such as `/Applications/My Tools/`)
  is split at the space and can be missed (known limit of reading the command line).
- Helper processes under an agent (MCP servers, language servers, `caffeinate`, shells) are not
  agent CLIs and never match; a helper whose program is literally named `claude`, `codex`,
  `opencode`, or `devin` would be shown as a sub-agent.
- `ps` fails or prints nothing usable: the previous map is kept and the next tick retries.
- A pid is reused after a session exits: only `running` sessions are read, so a stale pid is
  ignored.
- Many sessions: the process table is read once per tick and shared by all sessions.
- Agents Goko starts for its own ACP features (commit messages) are not under any session, so they
  never appear.

## Interfaces

- Protocol: `SessionInfo` gains `pid: Option<u32>`, serialized as `pid` (`null` before spawn, for a
  failed spawn, and from an older daemon through `#[serde(default)]`). The daemon sets it when the
  session becomes `running`. TS `SessionInfo` gains an optional `pid?: number | null`.
- Backend: `subagents.rs` with `spawn_poller(bridge, emit) -> SubagentState`, the command
  `subagents_get() -> HashMap<String, Vec<Subagent>>`, and the event `subagents://updated` carrying
  the same map. `Subagent` is `{agent: String, pid: u32, command: String}`.
- Frontend: `services/subagents.ts` (types and `subagentsService.get`), `features/subagents/queries.ts`
  (`useSubagents`, `useSessionSubagents(session)`), a `subagents://updated` listener in
  `services/sync.ts`, a `subagents` query key, and the indicator in
  `features/sessions/SessionItems.tsx`.

## Verification

- Rust unit tests: `ps` output parsing (including a command with spaces and an empty line);
  descendants at depth 1 and 3; the agent matcher (`devin`, an absolute `claude` path,
  `node /x/codex.js`, `bun /x/opencode`, a `zsh -c … devin …` wrapper not matched, a root process
  excluded); grouping into the map; the poller publishes only on change. Daemon and protocol tests:
  `pid` is `Some` for a running session, `None` for a failed spawn, and absent JSON reads as `None`.
- Vitest: the indicator for one, two of the same, and mixed agents; none for a session without
  entries or not running; `useSessionSubagents` hydrates from `subagents_get` and updates from the
  event.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`,
  `pnpm -C apps/desktop typecheck`, and the Vitest suite.
- Manual on the running app: in a Goko Claude session ask Claude to run a long `devin` command
  through its shell tool; the row shows `↳ devin` while it runs and loses it when it finishes.

## Related context

- [Architecture overview](../../../../llm/architecture/overview.md)

## Previous work

- [Agent activity: working, needs you, done](../2026-11-05-agent-activity/design.md)

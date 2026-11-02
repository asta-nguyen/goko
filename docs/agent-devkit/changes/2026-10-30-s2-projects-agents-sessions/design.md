# S2 Projects, agents, and sessions

Phase: S2

## Goal

Run several interactive coding agents side by side, grouped by the project each works on,
without losing any S1 terminal behavior. The app gains an Orca-style project sidebar —
persistent project navigation with an Add Project action, a per-project new-session
launcher, an agent picker, and a session list grouped by project in All Projects — using
[onorca.dev](https://www.onorca.dev/) and its
[first-session doc](https://www.onorca.dev/docs/first-session) as an information
architecture reference only (no pixel parity, no full Orca feature set). Exit check: two
different real agents run side by side and switching sessions stops neither. Implements
the S2 row of the [roadmap](../../roadmap.md); depends on every S1 plan (s1-01 through
s1-13) being archived, which the working tree shows as complete.

## Scope

In:

- An app-owned SQLite store (`goko.db` inside `GOKO_HOME`) with a minimal schema and
  `user_version` migrations: `projects`, `sessions` (metadata), `settings`.
- Project import/list/remove through the native directory picker (the existing
  `dialog:allow-open` capability; no new permission).
- Local agent adapters with PATH discovery: Claude Code (`claude`), Codex (`codex`),
  OpenCode (`opencode`), and a generic shell (`$SHELL`, always available), plus the S1
  free-form custom command kept as a picker option.
- Session metadata recording (project + agent identity, keyed by the daemon session id)
  and a project-grouped All Projects session sidebar with a filtered named-project view.
- A new-session composer with an agent picker, built with a vendored shadcn/ui `Select`
  (and `Button`) on the existing semantic dark/light tokens — not a redesign of S1.
- Persisted settings: `active_project_id`, `last_agent`.

Out (unchanged from the roadmap): Git/worktrees (S3), GUI reattach, terminal-emulator
snapshots, and `interrupted` reconciliation (S4), command palette, search, split panes,
settings screens, performance work (S5), packaging (S6). Also out: daemon or protocol
changes (none are needed), project file scanning, per-agent configuration beyond PATH
discovery, project renaming, session titles editing, and any promise of session
restoration after a daemon restart. The last selected session for each project is kept
only in UI memory while the app is open; it is not persisted across app restarts.

Process note: the link to this folder in `docs/agent-devkit/INDEX.md`, and any roadmap
link, are added only after the joint design-and-tasks approval.

## Behavior

Assumptions presented for the joint approval gate (consistent with current source):

- A project is a registered local directory; it does not need to be a Git repository.
- Registration and removal never modify or delete project files; removing a project
  never stops sessions.
- The daemon remains the sole PTY/process authority; project and agent association is
  app-owned metadata in SQLite, so the daemon protocol does not change.
- Every project has its own session list and terminal context. Selecting a project
  restores its last selected live session for the current app run; if it has none, the
  terminal area is empty until the user selects one. `All Projects` keeps the current
  valid selection. Switching project or session only changes the UI subscription; no
  PTY is stopped or detached.

**Shared constraints.** New UI uses the existing semantic tokens only — no hex colours
in components; TypeScript stays `strict` with no `any` without a comment; the UI never
builds a shell string (custom commands still go through the bridge's `shlex` split);
terminal bytes never enter React state, Zustand, or the query cache; no new Tauri
capability or permission is added; automated tests never spawn a real agent CLI.

**Projects.** Add Project opens the native directory picker. `project_add{path}`
canonicalizes the path (the directory must exist and be a directory) and inserts
`{id: uuid, name: basename, root: canonical path, created_at}`. Re-adding the same
canonical root returns its existing project. Distinct roots with the same basename
remain separate; each project row shows a truncated root path under its name so projects
can be distinguished. `project_list` returns every project in `created_at` order, each
with a `missing` flag computed at list time (root no longer exists). `project_remove{id}`
deletes only the row; `sessions.project_id` is nulled by `ON DELETE SET NULL`.

**Sidebar (Orca IA, Goko minimal).** The left column keeps its S1 role and gains, top
to bottom: a **Projects** section (`All Projects` row first, then one row per project —
name, truncated root path, live-session count, `missing` badge when the root is gone —
plus a new-session and remove action on each project row, and an `Add Project` button in
the section header), then the **new-session composer**, then the **Sessions** section.
The project rows scroll independently when the list is taller than the sidebar. Clicking
a row selects that project; its new-session action selects the project and focuses the
composer but does not launch a process. Remove is a two-step inline confirm inside the
row ("Remove <name>? Remove / Cancel"); the copy states that files and running sessions
are untouched.

**Selection and filtering.** Zustand gains `activeProjectId` (`null` = All Projects),
persisted to `settings.active_project_id` on every change and hydrated once at startup,
plus an in-memory `lastSelectedSessionByProject` map. A stored project id that no longer
matches a project falls back to All Projects and clears the key. Under a named project,
the session list keeps its existing flat layout and shows only sessions whose metadata
`project_id` matches. All Projects shows every daemon session, grouped in `project_list`
order by registered project; only projects with live sessions get a group. Each group
heading shows the project name and truncated root so projects with the same name remain
distinguishable. Sessions with missing metadata, a null `project_id`, or an id that no
longer matches a registered project appear in a final **Unassigned** group when
present. Session rows keep their agent label and daemon-owned
session state badge. Grouping is display-only: selecting a session remembers it for its
metadata project, including when selected from All Projects. Switching to a named
project selects its remembered session if that session is still in the daemon list;
otherwise it clears the terminal selection. Switching to All Projects retains the
current selection when it is still live. This session-selection map lasts only until
the app exits. Removing the active project falls back to All Projects without stopping
sessions; its former sessions appear in Unassigned.

**Composer.** The session context is the active project: under a project the cwd is the
project root (shown read-only) and Start sends `project_id` (with either an agent or
custom command), not `cwd`; under All Projects the user picks a directory as in S1 and
sends `cwd`, with no `project_id` — a directory chosen there never infers a project.
Both modes send the S1 `cols`, `rows`, and optional `title` fields. The agent picker is
a shadcn `Select` listing the discovered agents:
unavailable entries are disabled with "not found on PATH". `Custom command…` is the last
option and reveals the S1 command field (prefilled with `default_shell`). The default
selection is `settings.last_agent` when still available, else Shell. Start still sends
`cols: 80, rows: 24`, is disabled while pending or while input is missing, keeps the
double-click `inFlight` guard and the fieldset disable, and renders a `BridgeError` with
message and hint inline.

**Agents.** Adapters: `shell` (always available, argv = `[default_shell]`, using the
daemon's `/bin/zsh` fallback when `$SHELL` is unset), `claude`,
`codex`, `opencode` (argv0 named the same). `agent_list` resolves each on `PATH` (file
exists and has an execute bit) and returns `{id, label, available, path}`; `shell`
reports the resolved default shell. At `session_create` the backend resolves the agent
fresh — a real agent launches as argv `[resolved absolute path]` — and a successful
agent launch writes `settings.last_agent`.

**Session metadata.** After the daemon returns `SessionInfo`, `session_create` inserts
a `sessions` row `{id: daemon session id, project_id (nullable), agent, argv (JSON),
cwd, title, created_at}`. `session_meta_list` returns all rows; the sidebar joins them
to the daemon's `session.list` by id to filter and to render a small agent label on each
row. On successful session creation, the frontend invalidates `sessionMeta` so the new
row and agent label appear without waiting for another refresh. The daemon stays
authoritative for live state; rows whose session id is absent
from the daemon's list (daemon restarted) are hidden in the UI but retained in SQLite —
S4 owns reattach; S2 promises nothing about them. A `sessions` insert that fails never
fails `session_create` (the session runs; it shows as unassigned).

**Storage errors.** The store is opened during Tauri setup. If `goko.db` cannot be
opened or migrated, the app still starts with that storage error retained in managed
state; every storage-backed command fails with `{code: "storage"}`, the sidebar shows
the message where the project list would be, and daemon-backed features (sessions,
terminals) keep working. A settings write failure is ignored for
`active_project_id`/`last_agent` persistence (in-memory state still applies).

## Edge cases

- `goko.db` open or migration failure → `storage` error on storage commands; project UI
  degrades to an error note; `session_create` still creates sessions (metadata skipped).
- Adding the same directory twice, or via a different spelling/symlink → the canonical
  `root` UNIQUE constraint hits; `project_add` returns the existing project unchanged
  (idempotent, not an error). Different roots with the same basename remain separate
  and are distinguished by the truncated root path in the project row.
- Adding a path that does not exist or is not a directory → `invalid_argument`.
- A project root moved or deleted after registration → `missing: true` in `project_list`;
  the row gets a badge and its `+` is disabled; `session_create` with that `project_id`
  fails `invalid_argument`; Remove still works.
- Removing a project while its sessions run → sessions keep running (the daemon owns
  them); their metadata becomes unassigned and they appear in the All Projects
  **Unassigned** group.
  Removing a project that does not exist → `not_found`.
- Switching to a named project restores its remembered session only while that session
  remains in the daemon list; no prior selection or a vanished session leaves the
  terminal empty until the user selects a row. Switching to All Projects retains the
  current live selection. All PTYs keep running, and per-project selection memory is
  not restored after an app restart.
- `session_create` with both or neither of `command`/`agent`, both or neither of
  `project_id`/`cwd`, or an unknown `project_id` → `invalid_argument`/`not_found`; the
  daemon is never reached for rejected input. If a project is removed between
  validation and metadata insertion, the process remains alive and its metadata is
  unassigned.
- An agent not on `PATH` → shown disabled in the picker; `session_create` with it fails
  `invalid_argument` with a hint to install it or pick Shell. Same result when the
  binary disappears between `agent_list` and Start (resolution happens at create).
  The generic shell is always available.
- Daemon disconnect → the S1 banner and disabled controls still apply; project data is
  unaffected because it is app-owned. Daemon restart → the session list is empty, so
  sidebar session rows disappear (metadata rows are kept but hidden); the app offers no
  reattach in S2.
- Session switching → `TerminalView` mounts/unmounts as in S1; the daemon keeps every
  session's PTY and replay buffer; only the visible subscription changes.
- A session created by an older app build or with metadata skipped (storage failure)
  has no `sessions` row → its terminal remains selected and it appears in the All
  Projects **Unassigned** group; it is not listed under a named project.
- A `sessionMeta` result already loaded before a session is created → the successful
  create invalidates that query, so the project filter and agent label refresh promptly.
  If metadata insertion failed, the session remains unassigned and is not remembered
  as a project selection.
- Dialog cancelled on Add Project → nothing changes (same rule as the S1 picker).
- A stored `active_project_id`/`last_agent` that no longer resolves → ignored and
  cleared/defaulted, never an error.

## Interfaces

Tauri commands (all new ones added to `handler()`; no capabilities change):

- `project_add { path } -> Project`, `project_list -> Project[]`,
  `project_remove { id } -> ()`.
- `agent_list -> AgentInfo[]` (`{id, label, available, path: string | null}`).
- `session_meta_list -> SessionMeta[]` (`{session_id, project_id: string | null,
  agent, argv: string[], cwd, title, created_at}`).
- `settings_get { key } -> string | null`, `settings_set { key, value } -> ()`.
- `session_create` accepts optional `agent`, `command`, `project_id`, `cwd`, and `title`,
  plus the S1 `cols` and `rows` fields. Exactly one of `agent`/`command` and exactly
  one of `project_id`/`cwd` is supplied. Existing S1 `command`+`cwd` callers keep
  working; returns `SessionInfo` unchanged.

New backend modules: `src-tauri/src/store.rs` (`Store` over `rusqlite`; `Project`,
`SessionMeta`; `MIGRATIONS` applied through `PRAGMA user_version`;
`PRAGMA foreign_keys = ON`; file `goko.db` mode 0600) and `src-tauri/src/agents.rs`
(`AgentInfo`, `discover`, `argv_for`). `Code::Storage` in `error.rs` serializes as
`"storage"`. Schema v1: `projects(id TEXT PRIMARY KEY, name TEXT NOT NULL,
root TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL)`;
`sessions(id TEXT PRIMARY KEY, project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
agent TEXT NOT NULL, argv TEXT NOT NULL, cwd TEXT NOT NULL,
title TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL)`;
`settings(key TEXT PRIMARY KEY, value TEXT NOT NULL)`.

Frontend: `services/types.ts` gains `Project`, `AgentInfo`, `SessionMeta`; new
`services/projects.ts`, `services/agents.ts`, `services/settings.ts`;
`services/session.ts` extends `CreateSessionInput` and adds `meta()`;
`services/errors.ts` adds the `storage` code; `queryClient.ts` adds `projects`,
`agents`, `sessionMeta` keys; `stores/ui.ts` adds `activeProjectId`/`setActiveProject`
and the in-memory `lastSelectedSessionByProject` map; `useCreateSession` invalidates
`sessionMeta` after success;
new `features/projects/{queries,ProjectList}.tsx`; `NewSessionForm`, `SessionList`, and
`App` reworked per Behavior. All Projects passes the registered projects to
`SessionList` for group headings; the named-project view keeps the existing flat rows.
shadcn/ui vendored: `src/lib/utils.ts` (`cn`),
`src/components/ui/{select,button}.tsx`, with shadcn Tailwind utilities mapped through
`@theme inline` to Goko tokens (`--color-background: var(--bg)`,
`--color-foreground: var(--text)`, `--color-popover: var(--panel)`,
`--color-primary: var(--accent)`, `--color-primary-foreground: var(--bg)`,
`--color-muted-foreground: var(--muted)`, `--color-destructive: var(--danger)`,
`--color-input: var(--border)`, `--color-ring: var(--accent)`). Reuse the existing
`--color-border` mapping and adapt component surface classes to `bg-panel`/`bg-bg`;
do not introduce self-referential names such as `--border: var(--border)`. The existing
`--color-muted` and `--color-accent` mappings stay compatible with S1 classes. Add a
`@/` path alias in `tsconfig` and `vite.config.ts`. New dependencies: `rusqlite`
with `bundled`, `uuid` with `v4` for the desktop crate;
`@radix-ui/react-select`, `class-variance-authority`, `clsx`, `tailwind-merge`,
`lucide-react` for the frontend.

The daemon protocol is unchanged: `SessionInfo` gains no fields and `session.create`
keeps its S1 shape. `crates/daemon` and `crates/protocol` are untouched.

## Verification

- `cargo test -p desktop`: store unit tests (migration applies and is idempotent on
  reopen, dedup add, `ON DELETE SET NULL`, settings and metadata round-trips); agents
  unit tests (PATH scan against synthetic dirs, executability, first-hit resolution,
  unknown/missing agent errors); command tests (validation matrix, metadata written,
  `storage`/`not_found`/`invalid_argument` codes); wiring tests invoking every new
  command through Tauri's IPC path.
- `pnpm -C apps/desktop test`: sidebar filtering and All Projects view grouped by
  project-list order, duplicate project names distinguished by root, missing/null/stale
  metadata under Unassigned, empty groups omitted, agent labels and daemon session
  state shown on rows; missing badge and disabled `+`, two-step remove confirm,
  storage-error note, agent picker options and disabled entries, composer payloads for
  project vs All Projects vs custom, persisted and stale `active_project_id`,
  `sessionMeta` invalidation after create, per-project last-session restoration and
  empty-project selection behavior, session switch mount/dispose.
- `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`,
  `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, and the hex-colour
  grep on `src` — all green; `cargo test --workspace` shows the daemon suite untouched.
- Manual S2 checklist (recorded per item like s1-13): add two projects including a
  duplicate and a symlinked path; confirm All Projects groups each session under the
  correct project and shows its agent label and session state, with unassigned sessions
  under Unassigned; rename a project root and see `missing`; remove a project while its
  session runs and find it under Unassigned; select a
  terminal in each project and switch back and forth to confirm each restores its last
  selection while the others keep running; run two different real agents side by side
  and switch sessions repeatedly — neither stops
  (the roadmap exit check; if fewer than two real agent CLIs are installed, record
  which and mark the missing leg `not run`); start a custom command session; quit and
  relaunch to see the active project restored; kill the daemon and see the sidebar
  empty of sessions with no reattach claim; confirm dark/light tokens still render.

## Related context

None.

## Previous work

- [S1 contract](../../s1-contract.md) — the system this extends; its scope-outs are S2's scope.
- [roadmap](../../roadmap.md) — S2 row and exit check.
- [s1-02 UI shell](../archive/s1-02-ui-shell/design.md) ([tasks](../archive/s1-02-ui-shell/tasks.md)) — theme tokens and capabilities this UI builds on.
- [s1-12 UI session](../archive/s1-12-ui-session/design.md) ([tasks](../archive/s1-12-ui-session/tasks.md)) — the composer, list, and dialog permission being extended.
- [s1-13 Acceptance](../archive/s1-13-acceptance/design.md) ([tasks](../archive/s1-13-acceptance/tasks.md)) — the manual-checklist convention S2 reuses.

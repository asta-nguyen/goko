# S2 Projects, agents, and sessions

Phase: S2

## Goal

Run several interactive coding agents side by side, grouped under the project each works
on, without losing any S1 terminal behavior. The sidebar is one collapsible project tree:
each project is a parent and its sessions are children; sessions without a matching
project appear under **Unassigned**. Add Project registers an existing directory. A
project's `+` menu starts a discovered agent in that project, or opens an inline form for
a custom command. Orca is an information-architecture reference only, not a pixel-parity
target or a full feature set. The exit check remains two different real agents running
side by side while switching sessions stops neither. Implements the S2 row of the
[roadmap](../../../roadmap.md); depends on every S1 plan (s1-01 through s1-13) being archived.

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
  and a project tree with child sessions plus a final **Unassigned** group.
- Per-project agent menus that start an agent immediately; a custom command uses an
  inline command form. The existing semantic dark/light tokens and Lucide icons remain.
- The settings schema remains app-owned. `session_create` records `last_agent`, but the
  tree does not use it as a default; `active_project_id` is unused by this UI.

Out (unchanged from the roadmap): Git/worktrees (S3), GUI reattach, terminal-emulator
snapshots, and `interrupted` reconciliation (S4), command palette, search, split panes,
settings screens, performance work (S5), packaging (S6). Also out: daemon or protocol
changes, project file scanning, per-agent configuration beyond PATH discovery, project
renaming, session title editing, and any promise of session restoration after a daemon
restart. The tree has no All Projects filter, arbitrary-directory launcher, persisted
active-project selection, or per-project remembered-session map.

## Behavior

Approved behavior:

- A project is a registered local directory; it does not need to be a Git repository.
- Registration and removal never modify or delete project files; removing a project
  never stops sessions.
- The daemon remains the sole PTY/process authority; project and agent association is
  app-owned metadata in SQLite, so the daemon protocol does not change.
- Each project is a collapsible parent whose daemon-listed sessions appear beneath it.
  Selecting a session selects its terminal; expanding or collapsing a project changes
  only the tree. Every other PTY keeps running. There is no separate active-project
  filter or remembered session per project.

**Shared constraints.** New UI uses the existing semantic tokens only — no hex colours
in components; TypeScript stays `strict` with no `any` without a comment; the UI never
builds a shell string (custom commands still go through the bridge's `shlex` split);
terminal bytes never enter React state, Zustand, or the query cache; no new Tauri
capability or permission is added; automated tests never spawn a real agent CLI. The app
header uses the bundled Goko `>_` brand mark; other interface icons stay in Lucide.

**Projects.** Add Project opens the native directory picker. `project_add{path}`
canonicalizes the path (the directory must exist and be a directory) and inserts
`{id: uuid, name: basename, root: canonical path, created_at}`. Re-adding the same
canonical root returns its existing project. Distinct roots with the same basename
remain separate; each project row shows a truncated root path under its name so projects
can be distinguished. `project_list` returns every project in `created_at` order, each
with a `missing` flag computed at list time (root no longer exists). `project_remove{id}`
deletes only the row; `sessions.project_id` is nulled by `ON DELETE SET NULL`.

**Project tree.** The sidebar has a **Projects** heading and an **Add Project** action,
then one collapsible row per registered project. A row shows its name, truncated root,
session count, and a `missing` badge when its root is gone. Its `+` menu lists discovered
agents; unavailable agents and all launch actions are disabled when the project root is
missing, the daemon is disconnected, or a mutation is pending. Choosing an agent starts
it immediately in the project root. **Custom command…** opens an inline form prefilled
with `default_shell`; its Start action is also disabled while disconnected. Remove is a
two-step inline confirmation stating that files and running sessions are untouched.
Project rows and sessions share the sidebar scroll area.

**Sessions and selection.** Sessions whose metadata references a registered project are
children of that project. Missing metadata, a null project id, or an id that does not
match the project list goes in a final **Unassigned** group. Rows show the agent label,
title, selected state, and daemon-owned session state marker. Project rows toggle
expansion; selecting a session switches the visible terminal subscription and does not
stop or detach any PTY. Selecting a session in a collapsed project expands its parent.
There is no All Projects row, named-project filter, active project setting, or
per-project last-session memory. Projects with no sessions remain visible.

**Session launch.** There is no standalone composer. Selecting an agent from a project's
`+` menu sends `project_id`, agent id, `cols: 80`, and `rows: 24`; selecting a custom
command opens the inline command field and sends the same project context. The UI does
not launch a session from an arbitrary directory; register that directory as a project
first. Custom commands continue through the bridge's `shlex` parser, and the UI never
constructs a shell command string. Create errors remain visible with the message and
hint.

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
opened or migrated, the app still starts with that error retained in managed state and
storage-backed commands fail with `{code: "storage"}`. The project error remains visible,
while daemon sessions stay selectable under **Unassigned** if project or session metadata
cannot be read. A session can still run because the daemon owns its process and terminal.

## Edge cases

- `goko.db` open or migration failure → `storage` error on storage commands; the project
  error note remains visible and daemon sessions stay selectable under **Unassigned**;
  `session_create` still creates sessions (metadata skipped).
- Adding the same directory twice, or via a different spelling/symlink → the canonical
  `root` UNIQUE constraint hits; `project_add` returns the existing project unchanged
  (idempotent, not an error). Different roots with the same basename remain separate
  and are distinguished by the truncated root path in the project row.
- Adding a path that does not exist or is not a directory → `invalid_argument`.
- A project root moved or deleted after registration → `missing: true` in `project_list`;
  the row gets a badge and its launch menu is disabled; `session_create` with that
  `project_id` fails `invalid_argument`; Remove still works.
- Removing a project while its sessions run → sessions keep running (the daemon owns
  them); their metadata becomes unassigned and they appear in the tree's **Unassigned**
  group.
  Removing a project that does not exist → `not_found`.
- Expanding or collapsing a project does not change terminal selection. Selecting any
  session only changes the visible terminal subscription; all other PTYs keep running.
- `session_create` with both or neither of `command`/`agent`, both or neither of
  `project_id`/`cwd`, or an unknown `project_id` → `invalid_argument`/`not_found`; the
  daemon is never reached for rejected input. If a project is removed between
  validation and metadata insertion, the process remains alive and its metadata is
  unassigned.
- An agent not on `PATH` → shown disabled in the picker; `session_create` with it fails
  `invalid_argument` with a hint to install it or pick Shell. Same result when the
  binary disappears between `agent_list` and Start (resolution happens at create).
  The generic shell is always available.
- Daemon disconnect → the S1 banner remains; each launch menu and any open custom-command
  Start action is disabled, while existing session rows remain selectable. Daemon restart
  → the session list is empty, so sidebar session rows disappear (metadata rows are kept
  but hidden); the app offers no reattach in S2.
- Session switching → `TerminalView` mounts/unmounts as in S1; the daemon keeps every
  session's PTY and replay buffer; only the visible subscription changes.
- A session created by an older app build or with metadata skipped (storage failure)
  has no `sessions` row → it appears under **Unassigned** and remains selectable; it is
  not listed under a named project.
- A `sessionMeta` result already loaded before a session is created → the successful
  create invalidates that query, so the agent label and project parent refresh promptly.
  If metadata insertion failed, the session remains selectable under **Unassigned**.
- Dialog cancelled on Add Project → nothing changes (same rule as the S1 picker).
- Existing `active_project_id` or `last_agent` settings from earlier builds are ignored
  by the D3 UI and do not affect tree selection or launch defaults.

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
`agents`, `sessionMeta` keys; `useCreateSession` invalidates `sessionMeta` after success.
`features/projects/{queries,ProjectTree}.tsx` owns the nested project/session sidebar;
`App` mounts the tree beside `SessionControls` and `TerminalView`. The D3 UI does not use
`NewSessionForm`, `SessionList`, `activeProjectId`, or a per-project session-selection map.
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
- `pnpm -C apps/desktop test`: project parents with nested agent/session rows, expansion,
  selection and **Unassigned** behavior; missing roots; unavailable agents; project
  removal; storage-error sessions remaining selectable; disconnected launch controls;
  custom-command launch; `sessionMeta` invalidation; session switch mount/dispose.
- `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`,
  `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, and the hex-colour
  grep on `src` — all green; `cargo test --workspace` shows the daemon suite untouched.
- Manual S2 checklist (recorded per item like s1-13): register two existing directories;
  start different agents from each project row and confirm sessions appear beneath the
  right parent; collapse and expand parents; switch sessions repeatedly and confirm
  neither PTY stops; remove a project while its session runs and find it under
  **Unassigned**; move a root and see its missing badge and disabled launch menu; confirm
  unavailable agents and disconnected launch controls are disabled; start a custom
  command; verify dark/light tokens and the Goko header mark. Run two different real
  agents side by side for the roadmap exit check (if fewer than two real agent CLIs are
  installed, record which and mark the missing leg `not run`). No active-project restore
  or daemon reattach is promised.

## Related context

None.

## Previous work

- [S1 contract](../../../s1-contract.md) — the system this extends; its scope-outs are S2's scope.
- [roadmap](../../../roadmap.md) — S2 row and exit check.
- [s1-02 UI shell](../s1-02-ui-shell/design.md) ([tasks](../s1-02-ui-shell/tasks.md)) — theme tokens and capabilities this UI builds on.
- [s1-12 UI session](../s1-12-ui-session/design.md) ([tasks](../s1-12-ui-session/tasks.md)) — the composer, list, and dialog permission being extended.
- [s1-13 Acceptance](../s1-13-acceptance/design.md) ([tasks](../s1-13-acceptance/tasks.md)) — the manual-checklist convention S2 reuses.

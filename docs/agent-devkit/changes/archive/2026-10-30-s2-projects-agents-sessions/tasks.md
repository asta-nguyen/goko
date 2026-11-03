# S2 Projects, agents, and sessions tasks

## Design

[S2 Projects, agents, and sessions](design.md)

## Global Constraints

- A project is a registered local directory; it does not need to be a Git repository.
- Registration and removal never modify or delete project files; removing a project
  never stops sessions.
- The daemon remains the sole PTY/process authority; project and agent association is
  app-owned metadata in SQLite, so the daemon protocol does not change.
- Each project is a collapsible tree parent with its daemon-listed sessions as children.
  Selecting a session changes only the visible terminal; no switch stops or detaches
  another PTY. There is no active-project filter or remembered selection per project.
- New UI uses the existing semantic tokens only — no hex colours in components;
  TypeScript stays `strict` with no `any` without a comment; the UI never builds a shell
  string (custom commands still go through the bridge's `shlex` split); terminal bytes
  never enter React state, Zustand, or the query cache; no new Tauri capability or
  permission is added; automated tests never spawn a real agent CLI.
- A `sessions` insert that fails never fails `session_create` (the session runs; it
  shows as unassigned).

## Impact map

```text
Entry: apps/desktop/src/App.tsx::App and apps/desktop/src/features/projects/ProjectTree.tsx::ProjectTree; Tauri registration remains at apps/desktop/src-tauri/src/lib.rs::handler
Flow: ProjectTree → useProjectList/useSessionList/useSessionMetaList/useAgentList/useCreateSession → services/{projects,agents,session}.ts → Tauri commands and Store/agent discovery, or Bridge::call → daemon session.create → SessionManager::create → PtyProcess::spawn; the tree joins daemon sessions to metadata by session id/project_id; terminal output remains daemon replay → Tauri Channel → TerminalController → xterm
State changes: goko.db stores projects, session metadata, and settings; Zustand keeps only the selected session id in UI memory; daemon session state stays authoritative; no activeProjectId or per-project session-selection map
External effects: native directory picker using existing dialog:allow-open; PATH read for agent discovery; goko.db in GOKO_HOME; agent processes start through the daemon; no new Tauri capability or dependency for the tree UX changes
Change candidates: apps/desktop/src-tauri/{Cargo.toml,src/{lib,error,commands,store,agents}.rs,src/commands/tests.rs,src/wiring_tests.rs}; apps/desktop/src/{services/{types,errors,session,projects,agents,settings}.ts,queryClient.ts,stores/ui.ts,components/ui/{button,dropdown-menu}.tsx,styles/{tokens,index}.css,features/projects/{queries,ProjectTree}.tsx,features/sessions/{queries,SessionControls,StateBadge}.tsx,App.tsx} and relevant *.test.* files; ARCHITECTURE.md
Verification: cargo test --workspace; cargo clippy --workspace -- -D warnings; cargo fmt --check; pnpm -C apps/desktop test (including project-tree nesting, Unassigned fallback, disabled launch states, agent labels, and session-state markers); pnpm -C apps/desktop typecheck; pnpm -C apps/desktop build; semantic-token grep; manual S2 tree checklist with two different real agents side by side
Verified at: e075f405648ab0600627e7c8efb721c7efeec359
```

## Tasks

### Task 1 — App-owned SQLite store

Files: apps/desktop/src-tauri/src/store.rs (new: `Store`, `Project`, `SessionMeta`,
`MIGRATIONS`, `#[cfg(test)] mod tests`), apps/desktop/src-tauri/src/error.rs::Code,
apps/desktop/src-tauri/src/lib.rs, apps/desktop/src-tauri/Cargo.toml, Cargo.lock
Interfaces: produces `Store` and the `Project`/`SessionMeta` record types and
`Code::Storage` (serializes `"storage"`) consumed by Task 3; no command surface yet.
Change: add `rusqlite` (with `bundled`; version chosen during implementation and
pinned in `Cargo.lock`) and `uuid` (`v4`) to the desktop crate. `Store::open(home)` creates `goko.db` in
`GOKO_HOME` with mode 0600, sets `PRAGMA foreign_keys = ON`, and applies `MIGRATIONS`
through `PRAGMA user_version`. Schema v1 exactly per the design: `projects`,
`sessions` (with `project_id ... ON DELETE SET NULL`), `settings`. Methods:
`add_project` (UNIQUE root conflict returns the existing row), `list_projects`
(`created_at` order), `remove_project`, `insert_session_meta`, `list_session_meta`,
`get_setting`, `set_setting`. Open/migration/query failures become the storage error.
Verify: `cargo test -p desktop store` — a fresh open applies v1 and reopening is
idempotent; add then list round-trips; inserting a duplicate root returns the original
row (id, name unchanged); `remove_project` nulls `sessions.project_id`; settings
get/set round-trip; `insert_session_meta` then `list_session_meta` round-trips.
Progress: done
Result: `cargo test -p desktop store` — 5 store tests pass (open applies v1 mode 0600,
idempotent reopen, dedup add returns original row, `ON DELETE SET NULL`, settings and
metadata round-trips, retained open failure); `cargo clippy -p desktop -- -D warnings`
and `cargo fmt --check` clean. rusqlite pinned `=0.40.2` (0.41.1 was only 5 days old).

### Task 2 — Agent discovery and adapters

Files: apps/desktop/src-tauri/src/agents.rs (new: `AgentInfo`, `discover`, `argv_for`,
`#[cfg(test)] mod tests`), apps/desktop/src-tauri/src/lib.rs
Interfaces: produces `AgentInfo{id, label, available, path}` and agent→argv resolution
consumed by Task 3's `agent_list`/`session_create`.
Change: adapters `shell` (always available, argv `[default_shell]`, falling back to
`/bin/zsh` when `$SHELL` is unset), `claude`,
`codex`, `opencode` (argv0 of the same name). `discover(path_var)` scans each PATH
entry for a file that exists and has an execute bit, first hit wins, and reports
`{id, label, available, path}` for every adapter. `argv_for(agent, path_var,
default_shell)` re-resolves at launch: a real agent yields `[absolute resolved path]`;
an unknown id or missing binary is `invalid_argument` with a hint naming the adapter
and suggesting Shell.
Verify: `cargo test -p desktop agents` — synthetic PATH dirs: found agent is available
with its resolved path; absent agent is `available:false, path:null`; a non-executable
file is unavailable; two hits resolve to the first; `argv_for` succeeds for a present
binary and rejects a missing or unknown one; `shell` always resolves.
Progress: done
Result: `cargo test -p desktop agents` — 5 agents tests pass (resolved path, absent,
non-executable, first-hit wins, `argv_for` present/missing/unknown, shell always
resolves, empty PATH); `cargo clippy -p desktop -- -D warnings` clean.

### Task 3 — Commands and bridge wiring

Files: apps/desktop/src-tauri/src/commands.rs (add `project_*`, `agent_list`,
`session_meta_list`, `settings_get`, `settings_set`; extend existing `session_create`),
apps/desktop/src-tauri/src/lib.rs::handler (registration and managed store state),
apps/desktop/src-tauri/src/commands/tests.rs, apps/desktop/src-tauri/src/wiring_tests.rs
Interfaces: consumes `Store`, `AgentInfo`/`argv_for`, `Code::Storage`; produces the
full command surface (names, arg and return shapes) the frontend services call.
Change: open the store at `home` during setup and keep an open-failure result in
managed state so storage commands reject with `storage` while the daemon path keeps
working; register every new command in `handler()`. `project_add` canonicalizes and
requires an existing directory (`invalid_argument` otherwise); `project_list` adds
`missing` by checking each root; `project_remove` rejects unknown ids with
`not_found`. `session_create` accepts `command?`, `agent?`, `project_id?`, `cwd?`, `title?`, plus
the S1 `cols`/`rows`: exactly one of `command`/`agent` and exactly one of
`project_id`/`cwd` — under a selected project the caller sends `project_id` and omits
`cwd`, under All Projects it sends `cwd` and omits `project_id`, and either mode
carries an agent or a custom command (S1 `command`+`cwd` callers keep working). An
unknown `project_id` is `not_found` and a missing project root is `invalid_argument`
— all rejected before the daemon is reached. An agent launch uses `argv_for`, and on
daemon success the command inserts the session metadata row best-effort: an insert
failure is logged, never propagated — the process stays running and the metadata
appears unassigned, including when the project was removed between validation and
insertion. A successful agent launch writes `settings.last_agent`.
Verify: `cargo test -p desktop` — commands tests cover the validation matrix
(both/neither `command`/`agent`, both/neither `project_id`/`cwd`, unknown `project_id`,
unavailable agent — each rejected without spawning the daemon, checked via
`Home::spawns()`); a `shell` agent session records metadata and `last_agent`, while a
`cat` custom-command session records metadata without changing `last_agent`;
`project_*` and `settings_*` round-trip; a `goko.db` that cannot be opened makes
storage commands reject `storage` while `daemon_status` still answers; wiring tests
invoke every new command name through the real IPC path with the frontend's argument
shapes.
Progress: done
Result: `cargo test -p desktop` — 56 tests pass. commands tests cover the validation
matrix (both/neither `command`/`agent`, both/neither `project_id`/`cwd`, unknown agent,
unknown `project_id` → `not_found`, vanished project root → `invalid_argument`, each
rejected before the daemon via `Home::spawns()`); a `shell` agent session records
metadata (`agent:"shell"`) and `settings.last_agent`; a `cat` custom-command session
records `agent:"custom"` without touching `last_agent`; a `project_id` session runs in
the canonical root; a `goko.db` that cannot open makes `project_id` creates reject
`storage` while `command`+`cwd` creates still succeed (metadata skipped).
`wiring_tests` invoke every new command through the real IPC path — `project_add`
(dedup on canonical root), `project_list` (with `missing`), `project_remove`
(`not_found` on unknown id), `agent_list`, `session_meta_list`, `settings_get/set` —
plus the broken-store scenario (`storage` rejections while `session_create`,
`daemon_status`, and `agent_list` still answer). Wire args are camelCase per Tauri's
default (`projectId`). `cargo test --workspace`, `cargo clippy --workspace --
-D warnings`, and `cargo fmt --check` all pass. Tauri `State` is not directly
constructible, so project/command logic lives in plain fns and `Bridge` methods the
tests call directly; the `#[tauri::command]` fns stay thin.

### Task 4 — Frontend services, query keys, and UI state

Files: apps/desktop/src/services/types.ts, apps/desktop/src/services/errors.ts::CODES,
apps/desktop/src/services/projects.ts (new), apps/desktop/src/services/agents.ts
(new), apps/desktop/src/services/settings.ts (new),
apps/desktop/src/services/session.ts::CreateSessionInput + sessionService,
apps/desktop/src/queryClient.ts::queryKeys,
apps/desktop/src/stores/ui.ts::useUi,
apps/desktop/src/features/sessions/queries.ts (`useAgentList`,
`useSessionMetaList`), plus `services/*.test.ts`, `stores/ui.test.tsx`,
`features/sessions/queries.test.tsx`
Interfaces: consumes Task 3's commands; produces the typed services, query keys, hooks,
and `activeProjectId` state that Tasks 6–8 use.
Change: add `Project`, `AgentInfo`, `SessionMeta` types; extend `CODES` with
`storage`; `projectService{list, add, remove}`, `agentService{list}`,
`settingsService{get, set}`; `CreateSessionInput` gains optional `agent` and
`project_id` with `command`/`cwd` relaxed to optional; `sessionService.meta()`;
`queryKeys` gains `projects`, `agents`, `sessionMeta`; `useUi` gains
`activeProjectId`/`setActiveProject` which persists `active_project_id` best-effort
through `settingsService.set`, plus a hydration helper mapping a stored id against the
project list (unknown → `null` and the key cleared), and an in-memory
`lastSelectedSessionByProject` map with an action to remember a session only after its
metadata row confirms the project association. Successful `useCreateSession` calls
invalidate `sessionMeta` so a preloaded metadata result cannot hide the new session.
Verify: `pnpm -C apps/desktop test` — `toBridgeError` keeps a `storage` code; each new
service calls its command with the documented args (mocked `invoke`); `useAgentList`
and `useSessionMetaList` load through their query keys; successful create invalidates
`sessionMeta`; `setActiveProject` writes the setting and restores the remembered
session for that project; an unknown stored id maps to `null` and is cleared.
Progress: done
Result: `pnpm -C apps/desktop test` — 87 tests pass; `pnpm -C apps/desktop typecheck`
clean. `toBridgeError` keeps `storage`; `projectService`/`agentService`/
`settingsService` invoke with documented args (`projectId` camelCase on the wire);
`sessionService.create` accepts the relaxed `CreateSessionInput` and `meta()` calls
`session_meta_list`; `useAgentList`/`useSessionMetaList` load through their keys;
`useCreateSession` invalidates `sessionMeta` on success; `setActiveProject` writes
`active_project_id` (best-effort, `''` clears) and restores the remembered session
only while live (All Projects retains a live selection); `hydrateActiveProject` maps
unknown/'' stored ids to `null` and clears the key. Shared `call` extracted to
`services/call.ts` (imported by session.ts and the three new services).

### Task 5 — shadcn/ui foundation

Files: apps/desktop/package.json, pnpm-lock.yaml, apps/desktop/tsconfig.json,
apps/desktop/vite.config.ts, apps/desktop/src/lib/utils.ts (new `cn`),
apps/desktop/src/components/ui/select.tsx (new),
apps/desktop/src/components/ui/button.tsx (new),
apps/desktop/src/styles/tokens.css, apps/desktop/src/styles/index.css
Interfaces: produces `cn`, `Select`, `Button`, the `@/` alias, and token aliases used
by Tasks 6–7; components are vendored by hand (no `components.json`, no CLI).
Change: add `@radix-ui/react-select`, `class-variance-authority`, `clsx`,
`tailwind-merge`, `lucide-react` (versions chosen during implementation and pinned in
`pnpm-lock.yaml`); `@/* → src/*` in `tsconfig` `paths` and Vite `resolve.alias`; map
shadcn's semantic utilities to the existing Goko tokens through Tailwind v4
`@theme inline` in `tokens.css`/`index.css` (`--color-background: var(--bg)`,
`--color-foreground: var(--text)`, `--color-popover: var(--panel)`,
`--color-primary: var(--accent)`, `--color-primary-foreground: var(--bg)`,
`--color-muted-foreground: var(--muted)`, `--color-destructive: var(--danger)`,
`--color-input: var(--border)`, `--color-ring: var(--accent)`), reusing the existing
`--color-border` mapping and keeping `--color-muted`/`--color-accent` compatible
with S1 classes, for both dark and `data-theme="light"`; never define
self-referential names such as `--border: var(--border)`; vendor `cn`, `Select`
(trigger/content/item styled with token classes only, surface classes adapted to
`bg-panel`/`bg-bg`), and `Button` (cva variants on tokens).
Verify: `pnpm -C apps/desktop build && pnpm -C apps/desktop typecheck` pass; the hex
grep on `apps/desktop/src` stays clean; the whole existing Vitest suite still passes
(`pnpm -C apps/desktop test`); Select interaction is exercised in Task 7's tests.
Progress: done
Result: `pnpm -C apps/desktop typecheck`, `build`, and `test` (87) all pass (the
>500 kB chunk warning is pre-existing); hex grep clean outside `tokens.css`. Deps
pinned exact: `@radix-ui/react-select@2.4.1`, `class-variance-authority@0.7.1`,
`clsx@2.1.1`, `tailwind-merge@3.7.0`, `lucide-react@1.61.0` (1.62+ too new for the
7-day rule). `@/*` aliased in `vite.config.ts` and `tsconfig.json` (`paths` with a
`./` prefix; TS 7 removed `baseUrl`). `@theme inline` gained the shadcn aliases onto
the existing primitives (background/foreground/popover/primary/secondary/
muted-foreground/accent-foreground/destructive/input/ring; no self-referential
names). Vendored `cn`, `Select` (trigger/content/item on token classes; no
`tw-animate-css`), and `Button` (cva variants; no `asChild` — `react-slot` was not in
the dependency list).

### Task 6 — Project sidebar section

Files: apps/desktop/src/features/projects/queries.ts (new: `useProjectList`,
`useAddProject`, `useRemoveProject`),
apps/desktop/src/features/projects/ProjectList.tsx (new),
apps/desktop/src/features/projects/ProjectList.test.tsx (new)
Interfaces: consumes Tasks 4–5 (`projectService`, `useSessionMetaList`,
`activeProjectId`, per-project terminal selection, `Button`); produces the Projects
section `App` renders.
Change: `ProjectList` renders the `All Projects` row first, then one row per project —
name, truncated root path, live-session count (daemon list joined to metadata), and a
`missing` badge when the root is gone — plus a `+` action that selects the project and
focuses the composer without launching a process, and a remove action with a two-step
inline confirm ("Remove <name>?" → "Remove" / "Cancel") whose copy states files and
running sessions are untouched. The project rows scroll independently when the list is
taller than the sidebar. `Add Project` sits in the section header, opens the directory
picker, calls `add`, then selects the result; adding an already-registered canonical
root returns the existing project, and distinct roots sharing a basename stay
distinguishable by their truncated paths; a cancelled dialog changes nothing. A
`storage` rejection renders an error note in place of the list. Mutations invalidate
`projects` (and `sessionMeta` on remove).
Verify: `pnpm -C apps/desktop test ProjectList` — rows and All Projects render;
selecting persists via `settingsService.set` and restores that project's remembered
live session; a `missing` project shows its badge and a disabled `+`; add calls
`projectService.add` with the picked path and a cancelled dialog does nothing; remove
takes the two-step confirm and invalidates both keys; a `storage`-coded list rejection
shows the note.
Progress: done
Result: `pnpm -C apps/desktop test ProjectList` — 10 tests pass; typecheck passes.
`features/projects/queries.ts` provides `useProjectList`/`useAddProject`/
`useRemoveProject` (remove invalidates `projects` + `sessionMeta`). `ProjectList`
renders the Projects section: `All Projects` row first, then rows with name,
left-truncated root (`direction:rtl`, `title` disambiguates same basenames), a live
count joined from daemon sessions × metadata, a `missing` badge with disabled `+`, a
per-row `+` (selects the project and focuses `#new-session`), and a two-step inline
remove ("Remove <name>? Files and running sessions are untouched." → Remove/Cancel).
`Add Project` in the header opens the picker, `add`s, and selects the result;
a cancelled dialog changes nothing; a `storage` list rejection shows the note in
place of rows. Removing the active project falls back to All Projects.
After review, successful removal returns keyboard focus to the stable All Projects
row when the removed row unmounts, unless the user moved to another project while the
removal was pending; Cancel returns focus to Remove. Removal only clears active
selection if the removed project is still active when the request resolves.

### Task 7 — New-session composer with agent picker

Files: apps/desktop/src/features/sessions/NewSessionForm.tsx::NewSessionForm,
apps/desktop/src/features/sessions/NewSessionForm.test.tsx
Interfaces: consumes `useAgentList`, `useDaemonStatus`, `useCreateSession`, `useUi`
(incl. `activeProjectId` and project root), `Select`/`Button`; produces the composer
`App` renders.
Change: the command field becomes a shadcn `Select` of discovered agents — unavailable
entries disabled with "not found on PATH" — ending in `Custom command…`, which reveals
the S1 command input prefilled with `default_shell`; the default option is
`settings.last_agent` when still available, else Shell. Under a project the cwd is its
root shown read-only and Start sends `project_id` with no `cwd` — `{agent,
project_id}` for an agent or `{command, project_id}` for a custom command; under All
Projects the directory picker stays and Start sends `cwd` with no `project_id` —
`{agent, cwd}` or `{command, cwd}`. Both modes keep `{cols: 80, rows: 24}` and the
optional `title`, plus the `inFlight` double-click guard, the fieldset disable, and
the inline `BridgeError` (message + hint).
Verify: `pnpm -C apps/desktop test NewSessionForm` — agents render with unavailable
entries disabled; the default selection follows `last_agent`/Shell; a project context
sends `{agent, project_id}` with no `cwd`, and `Custom command…` there sends
`{command, project_id}`; All Projects sends `{agent, cwd}` with no `project_id`, and
`Custom command…` sends `{command, cwd}` (the S1 payload preserved); both modes keep
`cols`, `rows`, and the optional `title`; two fast clicks call `create` once; a
rejected create renders message and hint.
Progress: done
Result: `pnpm -C apps/desktop test` — 103 tests pass; `pnpm -C apps/desktop typecheck`
clean. The command field became a shadcn `Select` of discovered agents — unavailable
entries `aria-disabled` with "— not found on PATH" — ending in `Custom command…`,
which reveals the S1 command input prefilled with `default_shell`. Default selection
is `settings.last_agent` (read once via `queryKeys.lastAgent`) when still available,
else Shell. Under a project the root renders read-only and Start sends
`{agent|command, projectId}`; under All Projects the picker stays and Start sends
`{agent|command, cwd}` — exact payloads asserted, `cols:80, rows:24` kept, double-click
sends one create, rejected creates render message + hint inline. The form carries
`id="new-session" tabIndex={-1}` for the row `+` focus. jsdom needed
scrollIntoView/pointer-capture/ResizeObserver stubs; the Radix select is opened in
tests via `keyDown ArrowDown` (jsdom PointerEvent lacks `button:0`).

### Task 8 — Filtered session list and app recomposition

Files: apps/desktop/src/features/sessions/SessionList.tsx::SessionList,
apps/desktop/src/App.tsx::App, apps/desktop/src/App.test.tsx,
apps/desktop/src/features/sessions/SessionList.test.tsx
Interfaces: consumes Tasks 4–7; produces the complete S2 screen.
Change: `SessionList` takes the already-filtered sessions plus each row's optional
agent label (from the metadata join), keeping the memoized row and state badge. `App`
composes the sidebar as Projects section → composer → Sessions, filters the list
(`null` = every daemon session including unassigned; a project id = only sessions
whose metadata `project_id` matches, hiding unassigned ones), keeps a session whose
metadata insert was skipped or failed selected and visible under All Projects as
unassigned, hydrates `active_project_id` once at startup
(stale → All Projects and the key cleared), falls back to All Projects when the active
project is removed, and changes the active terminal with project selection: restore
the project's last selected live session, or clear selection if none remains. Selecting
a metadata-associated session remembers it under its project, including from All
Projects; a session without metadata stays unassigned and is not remembered. The map is
in-memory for the current app run; other sessions remain alive.
Verify: `pnpm -C apps/desktop test App` — a project view shows only its sessions; All
Projects shows all including unassigned; a session with no metadata row stays selected
and appears only under All Projects; a stored valid `active_project_id` is applied and
a stale one
clears to All Projects; selecting another session still disposes/mounts the terminal
per the existing assertions; switching projects restores each project's last selected
live session, a project without one clears selection, All Projects retains the current
live selection, and no switch stops another PTY. `pnpm -C apps/desktop test SessionList`
— the agent label renders when metadata provides one, rows keep their memoization, and
selecting a metadata-associated row records its project selection.
Progress: done
Result: `pnpm -C apps/desktop test` — 115 tests pass; `typecheck` and `build` pass.
The build reports a 731.41 kB minified JS chunk (baseline: 602 kB), above Vite's
500 kB warning threshold. `SessionList` now takes `{sessions, metas}`
— the parent owns the query, so `App` passes the filtered list; each row shows the
metadata `agent` id as a small uppercase label when a row exists, keeps the memoized
row (a `useMemo`'d `Map` + `useCallback` `onSelect` keep props stable across unchanged
sessions), and clicking a row calls `rememberProjectSession` with the metadata
`project_id` (skipped for unassigned rows). `App` composes Projects → composer →
Sessions, filters `null` → all daemon sessions / id → `meta.project_id` match only,
hydrates `active_project_id` once on first projects load (stale id → All Projects and
`settings.set` clears the key), falls back when the active project leaves the list,
and renders the `sessions` query error inline. Tests cover filtering, unassigned
visibility, remembered-per-project restore + clear, All Projects retaining selection,
stored/stale `active_project_id`, removal fallback, and `service.stop` never called
during any switch (the controller mount/dispose log asserts only the view changes).
Creation selects the new session only when the active project id still equals the id
captured at submit (`null` for All Projects); switching to another project or All
Projects while create is pending preserves the current selection. App regression tests
cover both switching to another project and to All Projects before create resolves. A
separate App test simulates reconnecting to a daemon that returns an empty session list
and confirms the sidebar shows no sessions or Stop control.

### Task 8a — Group All Projects sessions by project

Files: apps/desktop/src/App.tsx::App,
apps/desktop/src/features/sessions/SessionList.tsx::SessionList,
apps/desktop/src/App.test.tsx,
apps/desktop/src/features/sessions/SessionList.test.tsx
Interfaces: consumes the project list, daemon sessions, and session metadata already
loaded by Task 8; produces display-only group headings while preserving the existing
filtered named-project view and row selection behavior.
Change: In All Projects, show one group for each registered project that has live
sessions, in `project_list` order. Each heading displays the project name and truncated
root so projects with duplicate names remain distinguishable. Show sessions with
missing metadata, a null `project_id`, or an id that no longer matches a registered
project in a final `Unassigned` group when any exist. Omit empty project groups. Keep
each memoized session row's agent label and daemon session-state badge unchanged. A
named-project view remains flat and filtered to that project's sessions; grouping never
changes the selected terminal or stops a PTY.
Verify: `pnpm -C apps/desktop test SessionList App` — two project groups render in
project-list order with name/root labels; unassigned sessions appear last; empty
projects do not render; each row retains its agent label and session state; selecting a
named project still shows only its sessions in the existing flat layout and keeps
current selection semantics.
Progress: done
Result: `pnpm -C apps/desktop test SessionList App` — 18 tests pass, covering group
order, project root labels, null/missing/stale project metadata under Unassigned,
omitted empty groups, agent labels, state badges, and the flat named-project view.
`pnpm -C apps/desktop typecheck` and `pnpm -C apps/desktop build` pass; the build
retains the known large-chunk warning.

### Task 8b — Project tree UX resilience and brand mark

Files: apps/desktop/src/features/projects/ProjectTree.tsx,
apps/desktop/src/features/projects/ProjectTree.test.tsx, apps/desktop/src/App.tsx,
design.md
Interfaces: no new API, schema, or dependency; consumes the approved D3 project tree.
Change: Keep daemon sessions selectable under **Unassigned** when project storage fails;
disable agent and custom-command launch actions while the daemon is disconnected; use
the bundled Goko `>_` mark in the app header; synchronize the S2 design document with
D3 and these failure behaviors.
Verify: `pnpm -C apps/desktop test ProjectTree`, `pnpm -C apps/desktop typecheck`, and
`pnpm -C apps/desktop build`; full repository checks run during final review.
Progress: done
Result: `pnpm -C apps/desktop test ProjectTree` — 19 tests pass, including selectable
sessions after project storage errors and launch actions disabled during disconnects;
`pnpm -C apps/desktop typecheck` and `pnpm -C apps/desktop build` pass. The build retains
the existing large-chunk warning. Final review: `cargo test --workspace` (151 tests),
the full desktop suite (96 tests), `cargo fmt --check`, `cargo clippy --workspace -- -D
warnings`, `cargo build --workspace`, desktop typecheck, and desktop build all pass.

### Task 9 — S2 manual checklist and documentation handoff

Files: ARCHITECTURE.md; no code.
Interfaces: consumes Tasks 1–8, Task 8a, Task 8b, and D3.
Change: run the manual checklist from the design's `## Verification` — register two
existing project directories; start sessions from each project and confirm they appear
under the correct parent with agent and state markers; expand/collapse parents; move or
delete a root and confirm its missing badge and disabled launch action; remove a project
while a session runs and find the session under **Unassigned**; confirm selecting another
session does not stop a PTY; confirm unavailable agents and disconnected launch actions
are disabled; run two different real agents side by side and switch sessions repeatedly
— neither stops (the roadmap exit check: if fewer than two real agent CLIs are installed,
record which and mark the missing leg `not run`); start a custom command; check dark/light
rendering and the Goko header mark. Record each item pass/fail/not run with observations
in `Result`. As an implementation handoff item, update `ARCHITECTURE.md` (process
topology gains `goko.db`, agent discovery, the project↔session model and
daemon-vs-metadata authority, restart behavior with no S2 reattach, schema v1). Wiki
coverage is not applicable: `docs/llm/` does not exist, so `document-wiki`'s
`## Task updates` creates no wiki files and no `delta.md` is written.
Verify: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo
fmt --check && pnpm -C apps/desktop test && pnpm -C apps/desktop typecheck` all pass;
every checklist item has an observed result recorded. D3 removes the former active-project
restore and All Projects checks.
Progress: partial
Result: `ARCHITECTURE.md` documents the S2 support-directory database, schema v1,
agent discovery, project/session metadata ownership, best-effort metadata writes, and
the no-reattach behavior after daemon restart. `docs/llm/` is absent, so wiki work is
not applicable. The manual checks below are historical D1/D2 results and do not verify
the D3 project tree. Manual checks used an isolated `GOKO_HOME` and disposable project
roots. Final automated verification passes: workspace Rust tests (151), desktop tests
(96), fmt, clippy, workspace build, desktop typecheck, and desktop production build; Vite
continues to report the existing over-500-kB bundle warning.

- Pass (manual): duplicate canonical roots remain one project; symlink spelling
  resolves to the canonical root; two roots named `app` register; moving a root shows
  `missing` and disables `+`.
- Pass (manual): project switching restores Codex/Claude sessions; both agents run
  side by side and remain live; removing Claude's project clears its metadata
  association without stopping it; custom command argv is shlex-split and unassigned;
  active project and last agent survive app relaunch; minimal PATH disables the three
  external agents with the expected hint while Shell remains available; `goko.db` is
  mode 0600.
- Pass (automated): App tests cover removed-project session visibility under All
  Projects and reconnecting with an empty session response, which removes session
  rows and the Stop control. The same-basename test checks both distinct full-root
  titles.
- Not run (manual visual): confirm the two same-basename roots are visually
  distinguishable after truncation; inspect All Projects group headings, agent labels,
  and state badges; inspect dark and light rendering.
- Not run (manual): kill the real daemon and observe the empty list. The automated App
  test simulates reconnect with an empty daemon response.

GUI checks stopped at the user's request to avoid cursor-driving automation; all app,
dev-server, and temporary-daemon processes started for those checks were stopped.
Task 9 remains partial until the visual checks and real-daemon restart check are run.

## Files inspected, no change

- `crates/protocol/src/{lib,message,session}.rs` — confirmed the protocol and
  `SessionInfo` need no change for S2.
- `crates/daemon/src/{session,server,pty,paths,main}.rs` — the daemon stays the
  PTY/process authority unchanged; `SessionManager` keeps its S1 semantics.
- `apps/desktop/src-tauri/src/{bridge,daemon_client,events}.rs` — connection, event
  pump, and spawn/reconnect wiring reused as-is.
- `apps/desktop/src-tauri/src/test_support.rs` — `Home`/`fake_daemon` fixtures the new
  tests reuse.
- `apps/desktop/src-tauri/capabilities/default.json` — `dialog:allow-open` already
  covers the project picker; no permission changes.
- `apps/desktop/src/features/terminal/{TerminalView.tsx,TerminalController.ts}` —
  verified that switching changes only the subscription; untouched.
- `apps/desktop/src/features/sessions/{SessionControls.tsx,StateBadge.tsx}` — reused
  unchanged.
- `apps/desktop/src/{main.tsx,services/sync.ts}` — provider and event-to-cache sync
  already do what S2 needs.
- `docs/agent-devkit/{roadmap.md,s1-contract.md,INDEX.md}` and the archived
  s1-02/s1-12/s1-13 `design.md`/`tasks.md` — scope, exit check, and file-shape
  conventions.
- `package.json`, `pnpm-workspace.yaml`, `crates/{protocol,daemon}/Cargo.toml`,
  `apps/desktop/src-tauri/tauri.conf.json` — workspace and dependency state.

## Baseline

Before S2 source edits: `cargo test --workspace` passed (135 tests),
`cargo clippy --workspace -- -D warnings`, `cargo fmt --check`,
`pnpm -C apps/desktop test` (73 tests), `pnpm -C apps/desktop typecheck`, and
`pnpm -C apps/desktop build` all passed. The production build reported its existing
warning that the JavaScript chunk exceeds 500 kB after minification.

## Approval Gate

Status: approved

## Decision Log

### D1 — Project-owned terminal selection

Question: Should changing projects keep the currently selected terminal or restore a
terminal associated with the newly selected project?
Decision: Each project has its own session list; switching to it restores the last
session selected for that project during the current app run, if that session is still
in the daemon list. If none remains, no terminal is selected until the user chooses
one. All Projects retains the current live selection. Other PTYs continue running.
Impact: Changes project-switch selection behavior and the Zustand/App wiring; selection
memory is in-process only and adds no SQLite setting or daemon/protocol change.
Confirmed by user: 2026-10-30.

### D2 — Group All Projects sessions by project

Question: Should All Projects keep a flat session list or group sessions by project?
Decision: All Projects shows live sessions grouped in project-list order, with project
name and truncated root in each heading; sessions with missing metadata, null
`project_id`, or no matching registered project appear in a final `Unassigned` group.
Empty project groups are omitted. The named-project view remains filtered and flat,
and each row continues to show the agent label and daemon session state.
Impact: Adds display-only grouping in `App`/`SessionList` and focused frontend
coverage; no daemon, protocol, metadata, or terminal lifecycle change. The revised
design and task plan are approved for implementation.
Confirmed by user: 2026-11-02.
Approval state: approved by user on 2026-11-02.


### D3 — Replace the project filter and composer with one project → session tree

Question: Should the sidebar keep separate Projects, New Session, and Sessions blocks, or
nest each project's agent sessions beneath it?
Decision: One tree (`ProjectTree`): every project is a collapsible parent with its live
sessions beneath it (agent label, title, state dot); sessions with no matching project go
in a final `Unassigned` group. `＋` on a project opens an agent menu (unavailable agents
disabled) and starts the session in the project root at once; `Custom command…` opens an
inline form. This supersedes D1 and D2 and the `All Projects` row, the named-project
filter, `NewSessionForm`, and `SessionList`. Dropped: sessions in an arbitrary directory
without a project (add the folder as a project first) and the persisted
`active_project_id`/in-memory per-project selection (`activeProjectId`,
`lastSelectedSessionByProject`, `hydrateActiveProject`). The `last_agent` default is no
longer used by the UI.
Impact: Frontend only (`App`, `ProjectTree`, `SessionControls`, `StateBadge`, `ui.ts`,
`useCreateSession`, Vitest setup); no Rust, daemon, protocol, or SQLite change. The UI
stops reading `active_project_id` and stops using `last_agent` as a default; the backend
continues recording `last_agent` after an agent launch. Any old active-project setting is
ignored. `useCreateSession` awaits the metadata refresh before inserting the session so
it never flashes under Unassigned.
Confirmed by user: 2026-11-02.

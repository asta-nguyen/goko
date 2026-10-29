# s1-10 Frontend services, query cache and UI state tasks

## Design

[s1-10 Frontend services, query cache and UI state](design.md)

## Global Constraints

- Components never call `invoke` directly; everything goes through `sessionService` and the hooks.
- TanStack Query owns daemon data; Zustand owns UI-only state; neither holds terminal bytes.
- Query defaults: `staleTime: Infinity`, `refetchOnWindowFocus: false`, `retry: false`.
- TypeScript `strict`; errors are a discriminated union, not strings.
- Pin `@tanstack/react-query` v5 at implementation time and record it in `decisions.md`.

## Impact map

```text
Entry: no existing source; planned entry: apps/desktop/src/services/session.ts::sessionService and apps/desktop/src/features/sessions/queries.ts — pending approval
Flow: no existing flow; planned flow: component → hook → sessionService → Tauri invoke/Channel; Tauri events → startQuerySync → queryClient.setQueryData → hooks re-render — pending approval
State changes: in-memory query cache and Zustand UI store
External effects: Tauri event listeners (must be removable) — pending approval
Change candidates: apps/desktop/package.json, apps/desktop/vite.config.ts (test config), apps/desktop/src/services/{types,errors,session,sync}.ts, apps/desktop/src/queryClient.ts, apps/desktop/src/features/sessions/queries.ts, apps/desktop/src/stores/ui.ts, *.test.ts(x) beside them
Verification: pnpm -C apps/desktop test; typecheck
Verified at: no commit exists
```

## Tasks

### Task 1 — Vitest, shared types, errors, `sessionService`

Files: apps/desktop/package.json, apps/desktop/src/services/types.ts, apps/desktop/src/services/errors.ts::{BridgeError, toBridgeError, isBridgeError}, apps/desktop/src/services/session.ts::sessionService
Interfaces: consumes the Tauri commands from s1-09; produces the typed API used by Tasks 2–3 and s1-11/s1-12.
Change: add Vitest with the jsdom environment, `@testing-library/react`, and a `test` script. Mirror `SessionState`/`SessionInfo`. Implement `toBridgeError` for object, string, and nullish rejections. Implement each `sessionService` method as a thin typed wrapper; `subscribe` builds the `Channel`, splits the 8-byte seq off every message, and returns an unsubscribe function that calls `terminal_unsubscribe` once.
Verify: `pnpm -C apps/desktop test services` — with `invoke` mocked, each method calls the right command with the right payload; a `{code:"auth_failed",...}` rejection becomes that union member; a string and `undefined` rejection become `unknown`; `subscribe` decodes seq and bytes and its unsubscribe function is idempotent; `pnpm -C apps/desktop typecheck` passes.
Progress: todo
Result: none

### Task 2 — Query client, queries, mutations

Files: apps/desktop/src/queryClient.ts::queryClient, apps/desktop/src/features/sessions/queries.ts::{useSessionList, useSession, useDaemonStatus, useCreateSession, useStopSession}
Interfaces: consumes Task 1; produces the data hooks used by s1-11 and s1-12.
Change: add `@tanstack/react-query`. Create the client with the design's defaults and the two query keys. `useSession(id)` selects from the list so unchanged sessions keep their object identity. `useCreateSession` upserts the returned session into `['sessions']` on success; `useStopSession` calls `stop` and leaves the state change to events. Mutation errors are `BridgeError`.
Verify: `pnpm -C apps/desktop test queries` with a fresh client and mocked service — the list loads and is ordered by creation; `useSession(a)` returns the same object after session `b` changes; create success inserts the session and `isPending` is true while in flight; a rejected create exposes a `BridgeError`; `client.getDefaultOptions()` has the three defaults.
Progress: todo
Result: none

### Task 3 — Event-to-cache sync

Files: apps/desktop/src/services/sync.ts::startQuerySync
Interfaces: consumes Task 2 and the `session://*` / `daemon://status` events (s1-09); produces a live cache.
Change: register the three listeners. Session events `await cancelQueries(['sessions'])` then upsert by id (insert if unknown). `connected:true` invalidates both keys; `connected:false` sets `['daemon-status']` to `connected:false`. Return a stop function that removes every listener; a second call returns the same stop function.
Verify: `pnpm -C apps/desktop test sync` with a fake event source — an update changes one session without replacing the others; an unknown id is inserted; an event during an in-flight fetch is not overwritten when the fetch resolves; `connected:false` then `true` triggers exactly one refetch of each key; the stop function leaves zero listeners; a second `startQuerySync` registers none.
Progress: todo
Result: none

### Task 4 — UI store and selection

Files: apps/desktop/src/stores/ui.ts::{useUi, useSelectedId}
Interfaces: consumes `useSessionList` (Task 2); produces `selectedId`, `select(id)`, and `useSelectedId()`.
Change: Zustand store holding only `selectedId`. `useSelectedId()` returns the stored id if that session is in the list, else `null`, so no clearing logic is needed when a session vanishes.
Verify: `pnpm -C apps/desktop test ui` — `select(a)` then `useSelectedId()` returns `a`; after the list loses `a` it returns `null` without calling `select`; selecting an unknown id returns `null`; the store holds no session data (shape check).
Progress: todo
Result: none

## Files inspected, no change

- None — depends on the frontend scaffold (s1-02) and commands (s1-09), which do not exist yet.

## Approval Gate

Status: pending

## Decision Log

- Before approval: TanStack Query added at the user's request (UI-only Zustand store, daemon data in the query cache); plan grew from 2 to 4 tasks. Gate stays pending.

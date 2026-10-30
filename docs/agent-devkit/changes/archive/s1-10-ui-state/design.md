# s1-10 Frontend services, query cache and UI state

## Goal

A typed frontend API over the Tauri commands, with TanStack Query owning data that comes
from the daemon (session list, daemon status) and Zustand owning UI-only state. Daemon
events are written straight into the query cache, so views stay fresh without polling.
No components. Implements the frontend rules in the [S1 contract](../../../s1-contract.md).

## Scope

In: Vitest setup, `services/{types,errors,session,sync}.ts`, `queryClient.ts`,
`features/sessions/queries.ts`, `stores/ui.ts`. Out: components (s1-11, s1-12). Depends
on: s1-02, s1-09.

## Behavior

- **Types:** `SessionState` and `SessionInfo` mirrored by hand in `services/types.ts`
  (generation from Rust is deferred until the type surface grows).
- **Errors:** `BridgeError` is a discriminated union on `code` (codes from s1-09 plus
  `unknown`). `toBridgeError(e)` normalizes any rejected `invoke`; `isBridgeError` is a
  type guard.
- **sessionService:** `create({command, cwd, cols, rows, title?})`, `stop(id, force)`,
  `list()`, `write(id, data: string)`, `resize(id, cols, rows)`,
  `subscribe(id, onBytes(seq, data: Uint8Array)) -> Promise<() => void>`, `status()`.
  `subscribe` builds the Tauri `Channel`, decodes the 8-byte seq prefix, and its returned
  function unsubscribes. Input is a string; xterm's `onData` is the only input in S1.
- **Query client:** one `QueryClient` with `staleTime: Infinity`,
  `refetchOnWindowFocus: false`, `retry: false` (the daemon pushes changes and the bridge
  already reconnects). Keys: `['sessions']` → `SessionInfo[]` in creation order,
  `['daemon-status']` → `{connected, default_shell, home}`.
- **Hooks:** `useSessionList()`, `useSession(id)` (selected from the list, same object
  identity when that session did not change), `useDaemonStatus()`, `useCreateSession()`
  (mutation; on success upserts the returned session into the cache),
  `useStopSession()` (mutation; the state change arrives by event). Mutation `error` is a
  `BridgeError`.
- **Event sync:** `startQuerySync(queryClient)` listens to `session://updated`,
  `session://exited`, `daemon://status`. For session events it awaits
  `cancelQueries(['sessions'])` and then upserts, so an in-flight fetch cannot overwrite
  newer data. `connected:true` invalidates `['sessions']` and `['daemon-status']`;
  `connected:false` sets status `connected:false` in the cache. Idempotent; the returned
  stop function removes all listeners.
- **UI store (Zustand):** `selectedId` and `select(id)`. `useSelectedId()` returns the
  stored id only if that session exists in the list, otherwise `null`.
- Terminal bytes never enter the query cache, Zustand, or component state.

## Edge cases

- Event for an unknown id → inserted (the daemon is the authority).
- Event during an in-flight list fetch → fetch cancelled, event applied, no stale overwrite.
- List query fails → hook exposes a `BridgeError`; the bridge reconnect then invalidates.
- Selected session disappears after a reload → `useSelectedId()` returns `null`.
- `startQuerySync` called twice → one set of listeners.
- Rejected `invoke` with a string, an object, or `undefined` → `unknown` code, no throw.

## Interfaces

`sessionService`, `BridgeError`, `queryClient`, `QueryClientProvider` wiring done in
s1-12, `useSessionList`, `useSession`, `useDaemonStatus`, `useCreateSession`,
`useStopSession`, `startQuerySync`, `useUi`, `useSelectedId`.
Dependencies added: `@tanstack/react-query` (v5, pinned), `zustand`, `@tauri-apps/api`,
dev: `vitest`, `jsdom`, `@testing-library/react`. No Query devtools.

## Verification

Vitest with mocked `@tauri-apps/api/core` and `/event`, a fresh `QueryClient` per test,
`renderHook` with a provider wrapper; `pnpm -C apps/desktop typecheck`.

## Related context

None.

# s1-11 Terminal controller and view tasks

## Design

[s1-11 Terminal controller and view](design.md)

## Global Constraints

- Terminal output never enters React state, props, Zustand, or the query cache.
- One xterm instance per mounted view; every listener, addon, and observer is disposed on unmount.
- Resize is debounced (~100 ms) and sent once per burst.
- Scrollback is bounded (5000 lines).

## Impact map

```text
Entry: apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView
Flow: TerminalView effect → TerminalController.mount → sessionService.subscribe → term.write; term.onData → sessionService.write; ResizeObserver → debounce → fit → sessionService.resize
State changes: none beyond the xterm instance owned by the controller
External effects: DOM listeners, ResizeObserver (all disposed)
Change candidates: apps/desktop/package.json, apps/desktop/src/features/terminal/{TerminalController.ts,TerminalView.tsx}, tests beside them
Verification: pnpm -C apps/desktop test terminal; typecheck
Verified at: no commit exists
```

## Tasks

### Task 1 — `TerminalController`

Files: apps/desktop/src/features/terminal/TerminalController.ts::TerminalController
Interfaces: consumes `sessionService.subscribe/write/resize` (s1-10); produces `mount(element)` and `dispose()`.
Change: add `@xterm/xterm` and `@xterm/addon-fit`. Implement the mount order, input forwarding, debounced resize, and idempotent dispose from the design, including unsubscribing when `dispose` runs before the subscribe promise resolves.
Verify: `pnpm -C apps/desktop test TerminalController` with a fake terminal and service — mount then dispose leaves zero registered listeners and one unsubscribe; mount, dispose, mount again does not duplicate written bytes; ten rapid resizes produce one `resize` call; a 0×0 element produces none; dispose before the subscribe promise resolves still unsubscribes; `resize` is called before `subscribe`.
Progress: done
Result: `pnpm -C apps/desktop test TerminalController` — 11 tests pass with fake `Terminal`/`FitAddon`/`ResizeObserver` and a fake service (resize is called before subscribe with the fitted 100x30; bytes reach `term.write` and `onData` reaches `write`; second mount ignored; mount+dispose leaves zero data listeners, no observing observer, disposed terminal and one unsubscribe even after two disposes; ten rapid observer events give one `resize`; unchanged size is not resent; a 0x0 element gives no fit/resize but still subscribes; dispose before the subscribe promise resolves unsubscribes on resolve, and the test fails when that branch is removed; a disposed controller cannot be remounted and a late chunk for the old subscription never reaches the new terminal); `pnpm -C apps/desktop typecheck` passes. Pinned exact: `@xterm/xterm` 6.0.0, `@xterm/addon-fit` 0.11.0. Rulings: `mount` returns a `Promise<void>` (callers may ignore it); a rejected `resize`/`write` is swallowed (the daemon rejects both on an exited session, and history must still show); a rejected `subscribe` prints `[goko] <message>` into the terminal rather than leaving an unhandled rejection; resize is skipped when the grid size is unchanged since the last send. Risk for s1-09/s1-13: `terminal_unsubscribe{id}` is keyed by session id only, so React StrictMode's dev mount, cleanup, mount of the same session can let the first controller's async unsubscribe cancel the second's subscription if the bridge handles them out of order. The s1-09 bridge agent has been told to make `terminal_subscribe`/`terminal_unsubscribe` order-preserving per session; nothing to change in the controller.

### Task 2 — `TerminalView`

Files: apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView
Interfaces: consumes Task 1 and `useSession` (s1-10); produces a full-height component for one session.
Change: create the controller in an effect keyed by `sessionId`, dispose in the cleanup, and render the empty state, exit-code banner, or error banner from the session's state. No `useState` for output.
Verify: `pnpm -C apps/desktop test TerminalView` — switching `sessionId` disposes the old controller before the new one mounts; an `exited` session shows its exit code; a `failed` session shows its error; no session shows the empty state; unmount disposes; `pnpm -C apps/desktop typecheck` passes.
Progress: done
Result: `pnpm -C apps/desktop test TerminalView` — 6 tests pass with a fake `TerminalController` (no session → empty state and no controller; running session mounts on the host `div` with no banner; switching `sessionId` logs new:a, mount:a, dispose:a, new:b, mount:b; unmount disposes; `exited` shows `code 3` and still mounts the terminal; `failed` shows its error); `pnpm -C apps/desktop typecheck` passes. `sessionId` is `string | null` (null = empty state). Banners use `bg-panel`/`text-muted`/`text-danger` tokens only; `@xterm/xterm/css/xterm.css` is imported in the view. Not wired into `App.tsx` (s1-12), so `pnpm build` does not yet bundle xterm or its CSS; real rendering is checked manually in s1-13.

## Files inspected, no change

- `apps/desktop/src/services/session.ts` and `features/sessions/queries.ts` (s1-10, archived) — consumed as-is.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.

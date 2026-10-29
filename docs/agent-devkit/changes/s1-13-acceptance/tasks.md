# s1-13 Vertical-slice acceptance tasks

## Design

[s1-13 Vertical-slice acceptance](design.md)

## Global Constraints

- A result is `pass` only if it was observed; anything not run is `not run` with a reason.
- Test directories and sessions are disposable; no real repository is modified.
- Fixes found here carry their own automated test where one is possible.

## Impact map

```text
Entry: apps/desktop (tauri dev) and crates/daemon (gokod) from s1-01–s1-12, not yet written — pending approval
Flow: no existing flow; planned flow: the 13-item checklist in design.md exercising UI → bridge → daemon → PTY — pending approval
State changes: ARCHITECTURE.md written; fixes as needed in earlier plans' files
External effects: runs real interactive programs (vim, less, top) in PTYs — pending approval
Change candidates: ARCHITECTURE.md; any file a failing item points to
Verification: cargo test --workspace; pnpm -C apps/desktop test; clippy; typecheck; manual checklist
Verified at: no commit exists
```

## Tasks

### Task 1 — Run the checklist and write `ARCHITECTURE.md`

Files: ARCHITECTURE.md
Interfaces: consumes everything from s1-01 through s1-12.
Change: run the 13 checklist items from the design on `tauri dev`, repeat items 1, 2, 9, 12, 13 on the debug binary, and record each as pass, fail, or not run with the observation. Fix or document every failure. Write `ARCHITECTURE.md` (topology, framing, PTY ownership, buffer and `seq`, the raw-replay limit, crash behavior from items 12–13) from the code as built, not from the design.
Verify: every checklist item has a recorded observed result in `Result`; `cargo test --workspace && cargo clippy --workspace -- -D warnings && pnpm -C apps/desktop test && pnpm -C apps/desktop typecheck` all pass.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on s1-01 through s1-12, which do not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.

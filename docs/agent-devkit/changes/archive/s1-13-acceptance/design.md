# s1-13 Vertical-slice acceptance

## Goal

Prove the S1 slice works end to end on a real Mac with real interactive programs, and
record the architecture as built. GUI behavior has no macOS WebDriver, so this is a
manual checklist with recorded results. Closes the [S1 contract](../../../s1-contract.md).

## Scope

In: running the checklist, fixing or documenting each failure, writing `ARCHITECTURE.md`
at the repo root. Out: packaging, benchmarks, GUI-restart reattach guarantees (S4).
Depends on: s1-01 through s1-12.

## Behavior

Run on `pnpm -C apps/desktop tauri dev`, then once from the debug binary
(`target/debug/desktop` with `gokod` beside it). Each item is recorded as pass or fail
with what was observed:

1. Launch → exactly one `gokod` process (`ps`), status dot connected.
2. Choose a directory, start `$SHELL` → prompt appears; `ls --color` shows colours.
3. `vim` a file: edit, `:wq`; screen redraws cleanly.
4. `less` on a long file: scroll, search `/`, `q`.
5. `top`: live updates, `q` restores the shell.
6. `sleep 100` then Ctrl-C → prompt returns.
7. Resize the window: `tput cols; tput lines` match the visible grid.
8. `echo "xin chào 你好 🚀"` renders; paste three lines runs three commands.
9. Stop a shell → badge goes `exited` with its code. Start `sh -c 'trap "" TERM; sleep 100'`,
   Stop → stays `stopping`; Force kill appears after 5 s and ends it.
10. Two sessions: run a ticking loop in A, switch to B and back → A's output kept
    arriving while hidden.
11. `yes` for 5 s, Ctrl-C → UI stays responsive, no stuck state.
12. Cmd+Q then `ps` → `gokod` and its sessions still alive; relaunch → list reloads.
    Observation only: raw replay may look wrong for full-screen apps (known S1 limit).
13. `kill -9` the daemon → disconnected banner, then reconnect with a fresh daemon and an
    empty session list.

`ARCHITECTURE.md` covers process topology, framing, PTY ownership, buffer/seq semantics,
the raw-replay limit, and the crash behavior observed in items 12–13.

## Edge cases

- A failed item becomes a code fix (with its test) or a documented limit; it is never
  dropped from the record.
- A step that cannot be performed (e.g. a missing tool) is recorded as `not run` with the
  reason, not as pass.

## Interfaces

None new.

## Verification

`cargo test --workspace`, `pnpm -C apps/desktop test`, `cargo clippy --workspace -- -D
warnings`, `pnpm -C apps/desktop typecheck` all pass after any fixes; the checklist
results are recorded in the task `Result`.

## Related context

None.

# Agent instructions

## Purpose

Goko is planned as a native desktop app for running interactive commands in a terminal owned by a separate Rust daemon. The approved S1 contract is in [s1-contract.md](docs/agent-devkit/s1-contract.md).

## Project layout

The approved S1 plan places the Rust protocol and daemon in `crates/protocol/` and `crates/daemon/`, and the Tauri desktop app in `apps/desktop/`. The s1-01 bootstrap created the Rust crate skeletons; s1-02 added the desktop app (Tauri 2, Vite, React, TypeScript `strict`, Tailwind v4 with semantic colour tokens in `src/styles/`).

## Verification

- `pnpm install` — passed with the empty `apps/*` workspace.
- `cargo build --workspace`, `cargo fmt --check`, and `cargo clippy --workspace -- -D warnings` — passed.
- `cargo test --workspace` runs the protocol and daemon tests (the daemon integration tests start the real `gokod` in a temp `GOKO_HOME`).
- `gokod --version` prints `gokod 0.1.0`; an unsupported arg prints the usage and exits 2; no args runs the daemon: it holds `daemon.lock` in `GOKO_HOME` (default `~/Library/Application Support/Goko`), serves `daemon.sock`, writes `daemon.token` and `daemon.log`, and exits cleanly on SIGTERM/SIGINT. A second `gokod` prints `gokod: already running` and exits 0.
- The observed checks are recorded in [s1-01 task results](docs/agent-devkit/changes/archive/s1-01-repo-init/tasks.md).

Root `dev`, `build`, and `typecheck` scripts target `apps/desktop`. `pnpm tauri dev` runs from `apps/desktop` (use `pnpm -C apps/desktop tauri dev`); Vite uses port 5173 with `strictPort`, so a stale dev server fails the launch. The root `test` script has no target until Vitest arrives in s1-10.

## Skills

- `brainstorm-feature`: `.agents/skills/brainstorm-feature/SKILL.md`
- `context-handoff`: `.agents/skills/context-handoff/SKILL.md`
- `document-wiki`: `.agents/skills/document-wiki/SKILL.md`
- `estimate-feature`: `.agents/skills/estimate-feature/SKILL.md`
- `implement-task`: `.agents/skills/implement-task/SKILL.md`
- `lean-audit`: `.agents/skills/lean-audit/SKILL.md`
- `plan-feature`: `.agents/skills/plan-feature/SKILL.md`
- `read-codebase-context`: `.agents/skills/read-codebase-context/SKILL.md`
- `react-best-practices`: `.agents/skills/react-best-practices/SKILL.md`
- `review-and-verify`: `.agents/skills/review-and-verify/SKILL.md`
- `setup-codebase`: `.agents/skills/setup-codebase/SKILL.md`
- `setup-openez`: `.agents/skills/setup-openez/SKILL.md`
- `systematic-debugging`: `.agents/skills/systematic-debugging/SKILL.md`
- `using-devkit`: `.agents/skills/using-devkit/SKILL.md`

## Code intelligence

Locate concepts with OpenEZ `code_query`, approximate filenames with FFF
fuzzy file find, identifiers or literals with FFF grep (fallback: `rg`),
and regex with `rg`. Expand callers and callees with OpenEZ `code_context`
(1–2 hops); confirm string, route, config-key, and case variants with FFF
multi-pattern grep (fallback: `rg`). Use OpenEZ `code_outline` before
reading a large file. Search results are navigation; read current source as
evidence. Query OpenEZ directly with the repository path; if unavailable,
unindexed, or irrelevant, continue with FFF or `rg` and direct reads.

## Delegating implementation to Devin

You are the supervisor and own the final outcome of every task. Delegate the coding to the Devin CLI with the free `swe-2-flash-max` model, and spend your own quota on planning, review, and decisions.

- **Run it:** `devin --model swe-2-flash-max --permission-mode dangerous -p --prompt-file <file>`. Do not pass `--sandbox`; it overrides the permission mode and blocks shell commands. Devin runs in the user's working tree, so tell it not to commit, stash, reset, clean, or touch unrelated uncommitted changes.
- **One task per run:** write the prompt to a file. Point it at `AGENTS.md`, the change's `design.md` and `tasks.md`, name exactly one task, and require a final report with the files changed, the commands run with pass/fail counts, any deviation, and open questions.
- **Parallel runs:** start several Devin runs at once only when their tasks touch disjoint files. Otherwise run them in order.
- **Verify yourself:** never accept a report on trust. Check `git status`, read the diff, and rerun the task's `Verify` commands. A missing file or a test filter that matches nothing is a failure.
- **Fix loop:** if a check fails, send Devin the exact failing output and ask for a fix. After three failed attempts on one problem, stop and report it to the user instead of retrying.
- **Stay in the plan:** follow the approved plan. If a task is unclear, or you want to change the approach or scope, ask the user before continuing.

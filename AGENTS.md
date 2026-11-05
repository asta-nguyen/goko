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

You are responsible for managing the final outcomes of tasks. Use the Devin CLI—specifically the `swe-2-max` model with full permissions—to execute these tasks. Your role is to oversee the results; if any issues arise, instruct the `swe-2-max` model to fix them until the task is complete. Stick closely to the plan, and if you are unsure about anything or wish to propose an approach, ask for clarification. Since the `swe-2-max` model is free to use, leverage it for as much work as possible to conserve your own quota. Can add multi subagent `swe-2-max` to implement parallel tasks.

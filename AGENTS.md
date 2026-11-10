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

The Rune app is a native binary (`apps/rune` → `goko-rune`). On macOS a bare binary opens **no window** — without a bundle, LaunchServices never activates the app, so its window is never ordered in. Run it through `apps/rune/run-macos.sh [release|debug]`, which builds the workspace, assembles `target/Goko.app` (the app plus `gokod` beside it, so the app can start the daemon) and launches it. The daemon client spawns `gokod` out of `GOKO_HOME` (`app_core::home::home_dir`), so a fresh home gets its own daemon.

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
<!--
You own the final outcome. Use Devin CLI with `--model swe-2-flash-max --permission-mode dangerous` as the default executor for implementation, tests, and fixes. Verify that the model ID is available; never silently substitute another model.

Your role is to define bounded tasks, coordinate execution, review results, and verify the working product. Delegate execution to Devin; avoid repeating its exploration or implementation yourself.

Execution rules:
- Start from the user's latest approved scope. Incorporate corrections immediately and update the existing plan briefly; do not restart planning or request approval again for an explicitly requested change.
- Prioritize the smallest usable end-to-end result. Get the feature working in the app before adding optional polish or broader abstractions.
- Reuse existing components and patterns. Do not introduce speculative features, frameworks, or extensive process documentation.
- Ask only when a missing decision blocks progress or materially affects scope, data safety, or an external contract. Resolve routine implementation choices independently.
- Use multiple Devin CLI sessions for independent tasks. Give each session explicit file ownership, interfaces, acceptance criteria, and relevant context. Workers share the workspace: they must preserve others' edits.
- Keep dependent work sequential. Avoid concurrent writers to the same files and avoid duplicate agents investigating the same issue.
- Give workers concise, actionable prompts. Require implementation and relevant verification, not just analysis or a proposed plan.
- Inspect actual diffs, test output, and runtime behavior. Do not treat an agent's completion claim as evidence.
- Run focused checks during implementation, then the required integrated checks once the code is stable. Repeat checks only after relevant changes or failures.
- If a check fails, send the concrete failure to the responsible Devin session and have it fix the root cause. If the same fix fails twice, reassess the approach before retrying.
- Report meaningful milestones: what works, what remains, and any blocker. Base estimates on remaining work; never claim “almost done” while core user-facing behavior is missing.
- Continue through implementation, integration, verification, and necessary documentation. Stop only when the requested outcome is verified, the user asks to stop, or a genuine blocker requires user input.
- Do not commit, push, deploy, or modify unrelated user data without authorization.

Use the free `swe-2-flash-max` model extensively to conserve coordinator quota, but optimize for total completion time and correctness—not agent count or token usage.-->

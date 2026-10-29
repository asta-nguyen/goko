# s1-02 Tauri window and themed UI shell tasks

## Design

[s1-02 Tauri window and themed UI shell](design.md)

## Global Constraints

- TypeScript `strict`; no `any` without a comment.
- Colours come only from semantic tokens; no hex values in components.
- Capabilities grant no shell, fs, or http permission.
- Vite `strictPort: true` on 5173.

## Impact map

```text
Entry: no existing app source; planned entry: apps/desktop/src/main.tsx and apps/desktop/src-tauri/src/lib.rs::run
Flow: no existing frontend/Tauri flow; planned flow: tauri dev → Vite dev server → React App renders shell inside the webview
State changes: none found
External effects: opens a native window
Change candidates: all new — apps/desktop/** (including the Tauri development icon), pnpm-lock.yaml; root Cargo.toml (add member), Cargo.lock
Verification: pnpm typecheck/build; cargo check -p desktop; capabilities grep; manual tauri dev
Verified at: no commit exists
```

## Tasks

### Task 1 — Frontend scaffold (web only)

Files: apps/desktop/package.json, pnpm-lock.yaml, apps/desktop/index.html, apps/desktop/vite.config.ts, apps/desktop/tsconfig.json, apps/desktop/src/main.tsx, apps/desktop/src/App.tsx, apps/desktop/src/styles/index.css (Tailwind v4 per D1 in [decisions](decisions.md): no `postcss.config.cjs`; `@tailwindcss/vite` plugin in `vite.config.ts`)
Interfaces: produces a Vite + React + TypeScript (`strict`) + Tailwind app with scripts `dev`, `build`, `typecheck`, `tauri`.
Change: scaffold only; `App` renders one heading. Vite config sets port 5173 and `strictPort: true`. No Tauri code, no test runner.
Verify: `pnpm install && pnpm -C apps/desktop typecheck && pnpm -C apps/desktop build` pass; `pnpm -C apps/desktop dev` then `curl -s localhost:5173` returns the HTML.
Progress: done
Result: `pnpm install`, `pnpm -C apps/desktop typecheck`, and `pnpm -C apps/desktop build` passed. The Vite dev server on port 5173 returned the app HTML via `curl -s http://localhost:5173/`.

### Task 2 — Theme tokens and empty shell

Files: apps/desktop/src/styles/tokens.css, apps/desktop/src/styles/index.css (`@theme inline`), apps/desktop/src/App.tsx::App
Interfaces: produces the semantic colour tokens used by every later component through Tailwind colour names.
Change: define the tokens from the design in `tokens.css` with a dark default and a `[data-theme="light"]` override; map them with `@theme inline` in `index.css` (Tailwind v4, D1). `App` renders a header strip and a full-height main area using only token classes.
Verify: `pnpm -C apps/desktop build` passes; `grep -rE '#[0-9a-fA-F]{3,6}' apps/desktop/src --include=*.tsx` finds nothing; dark renders by default and `data-theme="light"` flips it (manual, visual).
Progress: done
Result: `pnpm -C apps/desktop build` passed on Tailwind 4.3.3 and the built CSS has `.bg-bg{background-color:var(--bg)}` and `.border-border{border-color:var(--border)}`; the hex grep found nothing. Review screenshots (headless Chrome, 1200×800, same built `dist`, `data-theme` toggled on `<html>`) show the dark shell by default and the light shell with `data-theme="light"`.

### Task 3 — Tauri shell opens the frontend

Files: Cargo.toml, Cargo.lock, apps/desktop/src-tauri/Cargo.toml, apps/desktop/src-tauri/build.rs, apps/desktop/src-tauri/tauri.conf.json, apps/desktop/src-tauri/icons/icon.{svg,png}, apps/desktop/src-tauri/capabilities/default.json, apps/desktop/src-tauri/src/lib.rs::run, apps/desktop/src-tauri/src/main.rs, docs/agent-devkit/changes/archive/s1-02-ui-shell/decisions.md
Interfaces: produces crate `desktop` and the window that hosts the frontend; later plans register commands in `run`.
Change: add Tauri 2 with identifier `dev.goko.app`, window per the design, `devUrl` and `frontendDist` wired to Task 1, restrictive CSP. Capabilities grant only minimal core permissions. Add `apps/desktop/src-tauri` to the workspace members. Record the pinned Tauri version and CSP in `decisions.md`.
Verify: `cargo check -p desktop` passes; `grep -E 'shell|fs:|http' apps/desktop/src-tauri/capabilities/default.json` finds nothing; `pnpm -C apps/desktop tauri dev` opens a window showing the shell (manual, visual).
Progress: done
Result: `cargo check -p desktop` passed; the capabilities grep for `shell|fs:|http` found nothing; `pnpm -C apps/desktop tauri dev` launched `target/debug/desktop` and System Events reported one window titled `goko`, 1200×800, with Vite answering 200 on port 5173. An earlier run failed with Vite's "Port 5173 is already in use" (a stale dev server), which matches the `strictPort` edge case.

## Files inspected, no change

- `package.json`, `pnpm-workspace.yaml` — existing root scripts and workspace pattern; the desktop package is new.

## Approval Gate

Status: approved

## Decision Log

None.

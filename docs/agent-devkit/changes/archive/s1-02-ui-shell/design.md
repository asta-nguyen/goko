# s1-02 Tauri window and themed UI shell

## Goal

A Tauri 2 window that hosts a React + TypeScript + Tailwind shell with dark and light
theme tokens. No daemon logic. Part of the [S1 contract](../../../s1-contract.md).

## Scope

In: `apps/desktop` Vite/React/TypeScript (`strict`)/Tailwind scaffold, semantic CSS
tokens, an empty shell layout, the Tauri crate `desktop` with minimal capabilities and
a restrictive CSP. Out: Vitest (s1-10), the dialog plugin (s1-12), daemon commands
(s1-09). Depends on: s1-01.

## Behavior

- Window title `goko`, default 1200×800, minimum 800×500, one window.
- Dark theme by default. Setting `data-theme="light"` on `<html>` switches to light.
  Tokens: background, panel, text, muted, accent, success, warning, danger, border.
- The shell is a header strip plus a full-height main area, using tokens only (no hex
  colours in components).
- Vite dev server uses port 5173 with `strictPort: true`, so a busy port fails loudly.
- Capabilities grant only the core permissions the main window needs. No shell, fs, or
  http permission is present.
- CSP is restrictive (`default-src 'self'`), allows Tauri IPC, and allows inline styles
  because xterm injects them. The exact string is recorded in `decisions.md`.

## Edge cases

- Port 5173 already in use → `tauri dev` fails with Vite's error, not a blank window.
- Missing frontend `dist` for a production build → build fails at `tauri build`, not at runtime.
- System in light mode: still dark by default until a setting exists (S5).

## Interfaces

CSS variables `--bg`, `--panel`, `--text`, `--muted`, `--accent`, `--success`,
`--warning`, `--danger`, `--border` exposed to Tailwind as colour names. Crate
`desktop` with `desktop::run()`; workspace member added to the root `Cargo.toml`.
`apps/desktop/package.json` scripts: `dev`, `build`, `typecheck`, `tauri`.

## Verification

- `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop build`.
- `cargo check -p desktop`; capabilities file contains no `shell`, `fs:`, or `http`.
- `grep` for hex colours in `.tsx` finds none.
- Manual: `pnpm -C apps/desktop tauri dev` shows the shell; toggling `data-theme` flips it.

## Related context

None.

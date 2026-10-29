# s1-02 decisions

## Tauri version and CSP

- Pin the Rust `tauri` crate to `=2.12.5`, `tauri-build` to `=2.7.4`, and the npm CLI to `2.12.4`.
- Include a small SVG-sourced terminal icon because Tauri's macOS development context requires an icon file; it uses the shell's dark and teal colors and can be replaced with final artwork in the release plan.
- Production CSP: `default-src 'self'; connect-src 'self' ipc: http://ipc.localhost; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost data: blob:; font-src 'self' data:`.
- Development `devCsp`: `default-src 'self'; connect-src 'self' ipc: http://ipc.localhost http://localhost:5173 ws://localhost:5173; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost data: blob:; font-src 'self' data:`. This permits Vite/HMR only in development.

## Tailwind version

### D1 — Tailwind v4 instead of v3

Question: Task 1 pinned `tailwindcss` 3.4.19 with a v3-style config (`tailwind.config.cjs`, `postcss.config.cjs`, `@tailwind` directives) and no recorded reason, while the design only says "Tailwind". Keep v3 or use v4?
Decision: Use Tailwind v4. Pin `tailwindcss` and `@tailwindcss/vite` to `4.3.3`, wire the plugin in `vite.config.ts`, and map the semantic tokens with `@theme inline` in `src/styles/index.css`. Remove `tailwind.config.cjs`, `postcss.config.cjs`, `autoprefixer`, and `postcss`.
Impact: Task 1 and Task 2 file lists; component class names and the token interface are unchanged. Tailwind v4 targets Safari 16.4+, which raises the minimum macOS webview.
Confirmed by user: 2026-10-29

## Sources

- [Tauri CSP](https://v2.tauri.app/security/csp/)
- [Tauri configuration and Cargo dependencies](https://v2.tauri.app/develop/configuration-files/)
- [Vite getting started](https://vite.dev/guide/)
- [Tailwind CSS with Vite](https://tailwindcss.com/docs/installation/using-vite)

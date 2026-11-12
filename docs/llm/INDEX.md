# Codebase wiki

Verified current behavior of this repository, grounded in source and tests.
Maintenance rules: [AGENTS.md](AGENTS.md).

## Pages

- [App icons](domains/app-icons.md) — Goko icon assets, macOS bundles, and Linux launcher packaging
- [Architecture overview](architecture/overview.md) — process topology, domains, and cross-domain dependencies
- [Desktop Editor](architecture/editor.md) — CodeMirror editing, Vim mode, Git content search, result navigation, and staging, commit, push, and ACP commit messages
- [Desktop Settings](domains/desktop-settings.md) — Settings page navigation, preference controls, and ACP agents
- [Desktop Theme](domains/desktop-theme.md) — dark and light palettes and theme preference behavior
- [Rune chrome](domains/rune-chrome.md) — goko-rune header, workspace tab strip, and session controls strip on mock data
- [Rune editor](domains/rune-editor.md) — native Files, Changes, History, Stashes, and Servers workspace over app-core with draft guards, worker isolation, and ACP commit messages

## Requirement prefixes

| Prefix | Domain |
|---|---|
| ICO | App icons |
| EDT | Desktop Editor |
| RNC | Rune chrome |
| RNE | Rune editor |
| SET | Desktop Settings |
| THM | Desktop Theme |

## Retired requirement IDs

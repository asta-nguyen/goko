# Codebase wiki

Verified current behavior of this repository, grounded in source and tests.
Maintenance rules: [AGENTS.md](AGENTS.md).

## Pages

- [App icons](domains/app-icons.md) — Goko icon assets, macOS bundles, and Linux launcher packaging
- [Architecture overview](architecture/overview.md) — process topology, domains, and cross-domain dependencies
- [Gokod daemon](domains/gokod-daemon.md) — transport (Unix socket or Windows named pipe), PTY driver, process stop, and GOKO_HOME resolution
- [Rune chrome](domains/rune-chrome.md) — project sidebar, live terminal tabs and panes, session controls, and native Settings and Appearance
- [Rune editor](domains/rune-editor.md) — native Files, Changes, History, Stashes, and Servers workspace over app-core with draft guards, worker isolation, ranked Context search, and ACP commit messages

## Requirement prefixes

| Prefix | Domain |
|---|---|
| GKD | Gokod daemon |
| ICO | App icons |
| RNC | Rune chrome |
| RNE | Rune editor |

## Retired requirement IDs

- `EDT-acp-commit-message`
- `EDT-codemirror-surface`
- `EDT-content-search`
- `EDT-discard-changes`
- `EDT-go-to-definition`
- `EDT-history-branch-filter`
- `EDT-history-detail`
- `EDT-history-table`
- `EDT-push`
- `EDT-stage-commit`
- `EDT-vim-save`
- `RNC-new-tab-menu`
- `SET-acp-agents`
- `SET-settings-page`
- `THM-dark-palette`

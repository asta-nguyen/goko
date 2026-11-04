# Wiki maintenance rules

This directory is the verified codebase wiki for this repository. It describes
current behavior only, grounded in source and tests read in the current run —
never proposed specs, plans, or process artifacts.

## Evidence rules

- Current source and tests are authoritative. File existence or Git history
  alone never establishes freshness; uncommitted working-tree changes count.
- Search results are navigation. Directly read source as evidence.
- Mark coverage `[ ]` undocumented, `[x]` verified against current source and
  tests, or `[~]` for either a `verification limit` (checks incomplete or
  required evidence inaccessible — the page may be correct) or a
  `confirmed content gap` (completed checks prove a material omission or a
  claim contradicting source). State which one and why; they are not
  equivalent, and a verification limit is never a refresh candidate.
- Never report unverified coverage as current.

## Logs

Any `LOG.md` in this directory is legacy. Do not read it, write it, or use it
to judge freshness; verify against current source instead.

## Requirements

Feature pages carry requirements in the `document-wiki` skill's format
(`### <PREFIX>-<slug>` ID headings, one `SHALL`, `#### Scenario:` blocks, and
`Evidence:` lines). Domain prefixes are registered in the
`## Requirement prefixes` table in [INDEX.md](INDEX.md); removed IDs go to its
`## Retired requirement IDs` list and are never reused.

## Links

Link internally with relative Markdown links. Every content page ends with a
`## Sources` section of exact source/test paths.

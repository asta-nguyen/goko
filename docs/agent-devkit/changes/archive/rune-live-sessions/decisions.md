# Rune live sessions decisions

### D1 — Settings port excludes Vim implementation

Question: Does this Settings port also implement Vim support in the native editor?
Decision: Port Settings (Appearance, Accounts, ACP agents and existing native
editor preferences); leave Vim implementation to a separate change. Do not add
an inert Vim switch.
Impact: Task 12 and its Settings verification exclude implementing Vim bindings.
Confirmed by user: 2026-11-10

# Delta

Merged into docs/llm/: 2026-11-13

Approved: 2026-11-13 — `⌘F` find bar over the focused terminal pane: the session's whole retained grid and scrollback (10 000 rows), wrapped rows joined, `FindState` reused, highlight plus next/prev; no daemon, protocol, or schema change

## ADDED Requirements

### RNC-terminal-find

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC (`rg -i 'find'` and the terminal requirements `RNC-terminal-links`, `RNC-terminal-selection`, `RNC-terminal-wheel`); no existing requirement covers searching terminal output. `RNE-code-surface` and the editor's contextual `⌘F` handle only the Editor view.

Requirement:

````md
### RNC-terminal-find

The system SHALL open a find bar over the focused terminal pane on `⌘F` in the Terminal view, search that session's whole retained grid and scrollback with rows the terminal wrapped joined (case-insensitive unless `Aa` is on), mark every match on screen with the current match stronger, and step through the matches, wrapping around, with `Enter`/`⇧Enter` or Next/Prev while scrolling the pane to each, keeping the keyboard in the query field until `Escape` closes the bar and returns it to the shell.

#### Scenario: find in scrollback

- GIVEN a pane whose scrollback holds `build failed` above the visible rows
- WHEN the user presses `⌘F` and types `build failed`
- THEN the bar shows `1 of 1`, the pane scrolls to the match, and the match is highlighted

#### Scenario: a match across a wrapped row

- GIVEN a line the terminal wrapped onto two rows
- WHEN the user searches for text that spans the break
- THEN it counts as one match and both rows show the highlight

#### Scenario: step and wrap

- GIVEN a query with three matches and the third current
- WHEN the user presses `Enter`
- THEN the first match becomes current; `⇧Enter` from there returns to the third

#### Scenario: keys stay out of the shell

- GIVEN the find bar open with its query focused
- WHEN the user types letters, then presses `Escape`
- THEN the letters edit the query and nothing is written to the session; `Escape` closes the bar, and the next typing reaches the shell

#### Scenario: no results

- GIVEN the find bar open
- WHEN the query matches nothing
- THEN the bar shows `No results` and no row is highlighted

#### Scenario: live output

- GIVEN the find bar open on a session that keeps printing
- WHEN new rows arrive that contain the query
- THEN the count updates within a fraction of a second, the marks follow, and the pane does not scroll by itself

Evidence: `apps/rune/src/term.rs`, `apps/rune/src/panes.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/find.rs`
````

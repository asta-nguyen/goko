# Delta

Approved: 2026-11-12 — play the macOS "Blow" sound when a session's activity becomes done or the session ends, only while the app window is unfocused, behind a `goko.sound_done` setting that defaults on

## ADDED Requirements

### RNC-done-sound

Page: `docs/llm/domains/rune-chrome.md`
Search: checked every page with prefix RNC; no existing requirement covers a completion sound.

Requirement:

````md
### RNC-done-sound

The system SHALL play a notification sound when a session's activity becomes done or the session ends (exited or failed), but only while the app window is unfocused and the `goko.sound_done` setting is enabled; the sound is the system "Blow" (`Blow.aiff`, or `Breeze.aiff` where Apple renamed it) through `afplay` on macOS, a synthesized tone through the first available player elsewhere, and silence when no player exists.

#### Scenario: agent turn finishes while unfocused

- GIVEN the app window is unfocused and the sound setting is on
- WHEN a running session's activity transitions to done
- THEN the done sound plays once

#### Scenario: focused window stays silent

- GIVEN the app window is focused
- WHEN a session's activity becomes done or the session ends
- THEN no sound plays

#### Scenario: setting disabled

- GIVEN `goko.sound_done` is off
- WHEN a session's activity becomes done or the session ends while unfocused
- THEN no sound plays

Evidence: `apps/rune/src/sound.rs`, `apps/rune/src/main.rs`, `apps/rune/src/sessions.rs`, `apps/rune/src/settings.rs`
````

# 3. Trash by default, permanent deletion as an explicit mode

Date: 2026-09-17. Status: accepted.

## Context

The app exists to delete things. Wrong verdicts will happen. Docker objects
and simulators have no Trash; files and folders do.

## Decision

Two modes, chosen per run and as a setting: Trash (default) and Permanent.
File-based actions move to the macOS Trash in Trash mode. Command-based
actions (`docker ... rm`, `xcrun simctl delete`) run the same in both modes
and the confirmation dialog says they cannot be undone. Every action is
planned, previewed, confirmed and logged.

## Consequences

Trash mode does not free space until the Trash is emptied; the UI says so.
Worktree removal in Trash mode is "move folder to Trash + git worktree prune"
rather than `git worktree remove`.

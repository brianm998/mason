# 0161: The weakening scan is narrowed and runs at submit_change

Status: accepted (waits for the owner: touches `protect.rs`, `selfguard.rs`)

## Context
25 tasks were sent back since 2026-11-08, many for lines that weaken nothing (`&|| true` closures, `.skip(1)`, `grep -c || true`, cleanup), each after a full test step.

## Decision
Scope each rule to the files where it means something, add the harmless idioms, treat lines in added files as notes, and run the scan in `change_validator`. Review mode refuses the submission; owner mode accepts with a note. Landing keeps the scan as backstop.

## Consequences
Fewer false findings; real ones come back before the tests. The scan is a heuristic over diff lines: it can miss a weakening written in an unusual language, which landing's identical scan will not catch either. Needed in protected files because the rules live there.

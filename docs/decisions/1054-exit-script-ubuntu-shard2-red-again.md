# 1054: fix s77 by stating what its runs are; leave s452 and s902 open

## Context

Run 45361042086, `exit-scripts (ubuntu-latest, 2)`. Only the log tail quoted in task #1099 was available; the full log could not be read, because the develop session has no network. The tail gives each script's `cause` line, which is the first line of its log matching `panicked at|FAIL|[Ee]rror|not ok` (`cause_of`, scripts/run-exit-scripts.sh).

| Script | Cause line in the tail | First failing line |
|---|---|---|
| s77 | `FAILED: the hand run was not seen waiting as interactive` | that line |
| s452 | `refused: error: refused: step.check-two.settings: directory must be a string ... (HTTP 422)` | not known: this is s19's expected refusal (scripts/exit/s19.sh:149), matched on `error`; the real failure is later in the full log, which was not read |
| s902 | `FAILED: the step did not stay in develop while the run waited` | that line, from scripts/exit/s787.sh, which s902 runs |

## Decision

- s77: pass `--origin person` on both hand runs, and wait for the observed holder instead of sleeping. No assertion is loosened and no sleep or deadline is lengthened. `scripts/exit/**` is protected; the owner approved both edits.
- s902: no change. docs/stages/s793.md records that s787 phase 1 also fails locally. Whether a step should outlive its time budget while a GitHub wait is credited is a follow-up for its own task.
- s452: no change to it or the seven scripts it wraps until its failing line is read.

## Alternative

Changing `origin_of` so a command with no terminal counts as a person: it would undo s552's ranking.

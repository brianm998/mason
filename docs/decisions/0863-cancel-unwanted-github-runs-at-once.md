# 0863: cancel a GitHub run at once when nobody will collect it

## Context
Superseded checks, checks of ended steps and of skipped tasks held GitHub runners until the next restart.

## Decision
Cancel at the moment the result becomes unwanted: in `run_with` for a superseded record, through a stop hook on agent checks, and in the reaper by also reading check records. Reuse `cancel_hold` so run, ref, record and hold end together.

## Consequences
Runner units return promptly. A check stopped this way reports `stopped` with a note; its step is over, so nothing reads it.

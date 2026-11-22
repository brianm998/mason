# 0874: the run_on test follows the placement modes

## Context
The test `run_on_takes_github_or_local_and_nothing_else` predates placement modes and rejected `remote`.

## Decision
Fix the test, not the code. It keeps its name and now accepts every value in `RUN_ON_VALUES`.

## Consequences
The known-red ledger entry clears. Adding a mode to `RUN_ON_VALUES` needs no test edit.

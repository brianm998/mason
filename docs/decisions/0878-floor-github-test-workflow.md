# 0878: the floor's GitHub test world carries the base workflow

## Context
The test world had no `.github/workflows/mason-run.yml` on its base, which the snapshot requires, so the run failed before reaching GitHub.

## Decision
Fix the test world only: push a minimal, non-push-triggered workflow to its base.

## Consequences
Rejected: relaxing the snapshot check or changing the test's expectation; the real repo carries the workflow, so the check is right.

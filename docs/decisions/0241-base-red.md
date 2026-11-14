# 0241: a red the base has too is not the task's

Stage s246. Status: accepted by the owner.

## Context
The base is consulted only for named tests that still fail after a rerun. On 2026-11-12 to 13 the Compose suite counted 0 tests whatever the change; 13 send-backs and 7 failed tasks named it, each after three develop rounds.

## Decision
- On a suite build that fails, a run that proves nothing, or a red run that names no failing test, the same suite is built and run at the merge base, once per (base commit, suite), cached in `baselines` with the facts the comparison needs.
- The base is red the same way when: its build failed with the same normalised first error; or its run was vacuous too; or its red run's normalised first error line is the same. Normalising strips paths, line and column numbers, hex ids and digits.
- Then the task is neither sent back nor charged a round. It waits in its test step, class `base-red`, text "`<suite>` is red on <base> too: <first error>". One urgent task, titled "`<suite>` is red on <base>: <first error>", is filed unless an open task or tracker issue already covers the same suite and normalised error.
- Every poll the base branch is fetched; when it has moved, the suite is checked once at the new tip (cached). When that is green the step runs again with the new base folded in.
- The wait is written to `base_red`, so a restarted daemon resumes it.

## Not done
Named failures that all fail at the base keep their existing excuse (the step passes). The comparison is by first error line only: two different breakages with the same first line are treated as one, and a breakage whose first line varies is never matched (the task is then sent back as before).

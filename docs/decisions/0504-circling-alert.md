# 0504: alert when a develop step keeps ending red on the same failing tests

**Decision.** Each full `run_check` of a develop step (no scope, the `tests` check) is recorded in the `check_runs` table: green, or red with its failing tests as `suite<TAB>test`. When the last 3 recorded runs of the task's current develop run are all red with the same non-empty set of failing tests, `alerts_now` raises `circling:<task>` (kind `needs_you`). It names the task, the tests, how long the streak has gone on, and whether the tests are in the known-red ledger (red on develop). It clears on a green run, a different set, or when the task leaves develop. The task's JSON carries a `circling` object, and the task counts in the header's needs-attention number and sits in the "Needs you" section while it holds.

**Why.** On 2026-11-16 three tasks spent 9 to 15 hours in develop with 5 to 10 full runs each, mostly red on one test that was red on develop. MASON alerted on stalls and on repeated step failures, not on one step going round in circles.

**Threshold.** `circling::RUNS = 3`. It is an alert threshold, not a budget: nothing is stopped, refused or limited. No file in `PROTECTED` changed, so no owner approval is needed.

**Not done / not verified.** Scoped runs (`changed`, `packages`, `tests`) are not recorded. A red run whose failing names cannot be read (a build failure, a timeout) is recorded with an empty set and so breaks a streak. The `rerun` ticket reads the same table; nothing here reads it for that yet. The Swift and Compose clients were not changed; they get the alert through the existing alerts list. I did not run the suite on a live daemon, only the new tests.

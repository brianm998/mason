# 0250: put the numbers the review counted by hand into the digest

Stage s255. Status: accepted by the owner.

## Context
A review of the week's events counted by hand: p90 claim-to-land time, send-backs per landed task by reason, retries by failure class, run times per suite and `run_check` calls per task. The digest had the median only.

## Decision
The digest adds, per project and in `--json`, only what the event log or the store records:
- claim-to-land median and p90 (nearest rank);
- send-backs by class (`test_red`, `build_red`, `conflict`, `weakening`, `other`) and per landed task, with the waits of class `base-red` shown as `base_red` (no send-back happens, a task waits);
- retries by the task's failure class (`code`, `outside`, `unclassified`), by hand and automatic;
- the test step's run time, median and p90.

What is not recorded is shown as `null` with the reason and is printed as "not recorded": the run time of each suite, the duration of `run_check` calls, and their count per task. Suite timings live in step logs, not in a table. Recording them (a row per suite run and per check) is a separate change.

## Not done
The classes are read from the text of a send-back reason, so a reason worded in a way the classifier does not know falls into `other`. Retries are classed by the failure class the task has when it is retried, which is `unclassified` for failures from before classes existed.

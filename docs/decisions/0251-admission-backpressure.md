# 0251: start fewer develop tasks while the check queue is long

Stage s256. Status: accepted by the owner.

## Context
On 2026-11-13, 27 tasks were in develop and 16 in testing, 22 of them waiting for a compile or test-suite slot, with 36 more queued to start. Each new develop task adds checks to the same queue and another branch to conflict with the base; about 30 send-backs that week were conflicts.

## Decision
- Admission does not start a project's next queued task while that project's waiting `run_check` and test-suite requests (waits of class `test-suite` or `test-small`) exceed a limit.
- The limit is the definition's `[admission] max_check_queue`; unset, twice the test-suite resource's count (the owner's runtime limit when there is one); 0 turns it off. A test-suite count of 0 does not hold everything.
- Tasks already admitted are unaffected. An urgent task is never held. A held task's activity says "held: N checks are waiting (limit M)".
- The change is in admission only, not in the broker or `sched.rs`.

## Not done
The count is of the project's own waits, not the machine's: another project's queue does not hold this one. Compile waits are not counted, only the test-suite ones. A held task shows its reason in its activity text but sends no event of its own.

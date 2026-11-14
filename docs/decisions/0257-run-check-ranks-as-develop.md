# 0257: a run_check ranks as the agent's step, not the test step

**Decision.** The slots a develop agent's `run_check` queues for are ranked by the agent's own step (`check_rank_step`). The check still asks for the test step's resources.

**Why.** Ranked as the test step, checks tied with real test steps on steps remaining, and priority and age decided; on 2026-11-13 four tasks in test waited behind develop agents. Work further along should go first. The reason for the old rank (develop agents timing out while their check waited) no longer holds: the queue wait is added to the agent's deadline (`DeadlineExtra`).

**Supersedes** the ranking part of the 2026-11-11 run_check change (`5e012882`); rejoining a run in flight and showing the wait are unchanged.

**Trust.** `jobs.rs` is not in `PROTECTED`; `scripts/exit/s262.sh` (exit tests) is. The owner approved this change on 2026-11-13 ("Yes, change it"). No gate, limit or test is weakened. (Corrected in s271: the first version said none of these paths was protected.)

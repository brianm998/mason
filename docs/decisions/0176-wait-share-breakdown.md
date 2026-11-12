# 0176: Show the wait breakdown in the wait_share finding rather than retune the scheduler

The finding "Tasks spend 61% of their time waiting" gave only totals. Waits on "a slot" are queue depth, waits on "the owner" are not the scheduler's, and resource waits are; they need different fixes. We keep the metric and threshold as they are (changing either to clear the finding would hide the problem) and add the per-reason shares to the evidence. A scheduler change waits for a finding that shows which reason dominates.

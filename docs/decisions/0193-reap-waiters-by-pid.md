# 0193: Reap a waiting request by its pid, not by poll age

A waiting `mason run` request was dropped after 10 s without a poll. Polls queue behind the broker's state lock, which a stalled disk can hold, so waiters lost their place (star #169, twice).

**Decision.** Every external lease records its requester's pid (outside requests too). A waiting lease with a pid is reaped only when that process is gone. One with no pid is reaped after `UNPOLLED_WAITER_REAP` (120 s) and the drop is logged. The granted branch is unchanged.

**Trust.** Edits no protected path.

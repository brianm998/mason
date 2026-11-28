# 1311: `stuck:long` judges working time, not capacity waits

**Decision.** `stuck:long` compares `in_state_ms - capacity_wait_ms` with its limit, and the time
rule does not fire while the task waits for a runner pool unit (`pool_wait`): `stuck:wait` judges
that wait. The check-round rule (`checks_max`) is unchanged and still fires whatever the waits.
The evidence shows the time in the state, the capacity wait, the working time and the limit.

**Why.** With the gha pool full of MASON's own jobs, develop runs waited 800 to 960 minutes with no
check round, and each one opened a health session that asked the owner whether to restart it.

**What counts as a capacity wait.** The wait classes `gha`, `gha-macos`, `gha-debt`, `compile`,
`test-suite`, `exclusive` and `node:<id>`, and the `environment` wait whose text starts "Waiting
for a node" (a run with no node unit in the broker's line). Waits for the owner, a pause, an agent
session, the base check or the environment do not count. Nor do the machine gates (`cpu`,
`thermal`, `memory`, `disk-stall`, `disk-floor`), even when they hold a compile request: the task
named slots, and those gates have findings of their own.

**The window** is the time since the task entered its state, the same as `in_state_ms`, not only
the newest step run. A daemon restart or a retry in the same state starts a new run without
resetting the state's age; waits of the earlier run would otherwise count as work.

**Where it comes from.** The `WaitStarted` and `WaitEnded` events that `wait_by_class` totals,
read for the window only (`Store::wait_events_since`), not a replay of the whole log.

**Not chosen.** A wait for a node that never comes is no longer raised by `stuck:long`. It is not
in the broker's line, so `stuck:wait` does not see it either; `stuck:task` still does when the
step makes no progress. `stuck:task` still uses the whole time in the state.

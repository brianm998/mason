# 0879: the gha ceiling test's job count follows the ceiling

The test expected five MASON jobs held under `gha_ceiling` 4 with no other workflows' jobs. `refresh_other` and the pool arithmetic cap MASON at 4, so the test was off by one; the scheduler is correct. The test now queues four jobs and asserts the fifth waits. Only the test in `crates/daemon/src/sched.rs` changed, which is a protected path and needs owner approval.

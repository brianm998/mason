# 0895: a check's wait is credited every second

A develop agent's check wait was credited to the step deadline every 30 s, but the watchdog kills at the deadline plus the credit, so budgets under 30 s ended the step while the check waited (#813). Check waits now credit every second (`CHECK_CREDIT_EVERY` in `jobs.rs`); a daemon restart loses at most a second of credit. `sched::CREDIT_EVERY` stays at 30 s for `mason run` waits, which are a separate path.

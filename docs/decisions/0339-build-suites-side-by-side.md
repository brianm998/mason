# 0339: a test step's build suites run side by side

**Decision.** Suites whose run needs only `compile`, with no `build` of their own, run side by side when a step selects two or more of them. Each runs on its own lease, and the broker caps how many run at once. Their results go through the step's usual loop, in order.

**Why.** Star's test step spent about ten minutes building five independent packages one at a time while compile slots sat free (2026-11-14). Running them concurrently costs nothing the scheduler does not already control, and handling the results in order keeps red handling, base-red checks and debts as they were. A suite that needs `test-suite`, `exclusive`, a device or an environment's resources is not run this way: those are where suites can disturb each other.

**Trust.** `crates/daemon/src/suites.rs`, `crates/daemon/src/sched.rs` and `scripts/exit/s345.sh` are in `PROTECTED`. The owner approved this change on 2026-11-14 ("now do the parallel builds within a step"). Nothing is weakened: every suite still runs, with what it asks for, and a red build still fails the step.

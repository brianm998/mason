# 0464: A waiting request reserves only the resources it is short of

Status: accepted (owner approval needed: `crates/scheduler/**` budgets and `crates/daemon/src/suites.rs` verdict logic)

Overnight on 2026-11-14/15 the compile queue stood 20-30 deep with 1-3 of 6 slots held, while test-suite was full. Every test step and landing re-test sent `{compile:1, test-suite:1}` for the `exit-scripts` suite (no `build`), and each waiter reserved a `compile` unit it was not short of.

**Planner rule.** A waiting request reserves the units of the resources it is short of: those where holders plus earlier reservations leave too little (its `Full` or `Behind` resource). Its other units are reserved only while it is first in line for the short resources, meaning no earlier waiter reserved any of them. So when the short resource frees, the first waiter finds its other units set aside and is granted in the same pass, and later requests cannot starve it. A waiter held for no resource (CPU, disk, memory, machine) reserves everything, as before. The exclusive rules are unchanged. The rule is written on `Blocked` in `plan.rs`.

**Suites.** `run_needs` now drops the step's `compile` for every suite, not only for ones with a `build`. A suite with no build does not compile; a suite that does compile without a build says `uses = { compile = 1 }` itself (the s345 side-by-side suites do). This was chosen over adding `uses` to `exit-scripts` because that suite already has `uses = { test-suite = 1 }`, and the step's `compile` was what got merged in. `scripts/run-changed-exit-scripts.sh` runs exit scripts that build through `mason run --needs compile`, which queue for compile themselves, so they do not need the suite to hold it.

`mason resources` and the monitor read the same plan; the mixed waiters now say `Full test-suite`, and compile-only requests say `Behind compile` only when compile is really used up.

Not verified: behaviour on the live daemon.

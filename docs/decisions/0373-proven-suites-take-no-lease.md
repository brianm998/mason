# 0373: a proven suite takes no lease

**Decision.** Before a test step leases a slot for one suite, it checks the evidence table. A suite already green for the step's exact tree takes no lease, and `run_suite` reports it reused, as it already would after being granted.

**Why.** Reuse needs nothing from the machine, but the lease came first, so a reused suite waited in the queue like real work. A restarted step is where this costs most (star #216, 2026-11-14: 25 minutes behind two whole-machine drains). The answer is the same either way. It just comes before the wait instead of after.

**Trust.** `crates/daemon/src/suites.rs` and `scripts/exit/s380.sh` are in `PROTECTED`. The owner asked for this on 2026-11-14 ("see if there are pipeline improvements to prevent this"). Nothing is weakened: evidence is only for the exact tree, and every suite without it still runs with what it asks for.

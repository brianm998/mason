# 0379: a task's test step may run only the tests its change needs

**Decision.** A suite that declares the `narrow_*` keys runs, in a task's test step, only the test units its change touches or that name a file it changes. The landing batch always runs it whole. A narrowed run is never evidence for the tree, never "green at base", never a run time, and never a cached baseline. This revises the rule written beside `dev_command`, that the test step always runs `command` whole, for suites that opt in.

**Why.** The test step's job is to find this change's failures early; the batch's job is to prove the combined tree before it lands. For a suite the batch runs anyway, a whole run per task repeats the batch's work. For a whole-machine suite, it also holds the machine for every small change (star #216, 2026-11-14). Opting in per suite keeps the default as it was. `narrow_never` and the "more than half" rule fall back to a whole run whenever the change is broad.

**Trust.** `crates/testing/**`, `crates/daemon/src/suites.rs` and `scripts/exit/s386.sh` are in `PROTECTED`. The owner asked for this on 2026-11-14 ("Yes, I want you to do both, here, now"). Nothing is skipped before landing: every test of a narrowed suite still runs in the landing batch, and a narrowed red is still red.

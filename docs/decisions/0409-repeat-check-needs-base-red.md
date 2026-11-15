# 0409: failing in many tasks is not proof that a test is red at the base

**Decision.** The repeat check no longer adds a test to the known-red ledger because it failed in 3 tasks within 7 days. It runs the test at the merge base, as the last-round check does, and adds it only if the test is red there.

**Why.** Being in the ledger excuses the test in every task, so a wrong entry lets broken code land. A test that checks each task's own change (format, lint, a registry of ids) fails in many tasks while the base is green. That made `workspace_is_fmt_clean` known red on 2026-11-15, and excused unformatted code for 29 days, until the owner lifted the quarantine.

**Trust.** `crates/daemon/src/suites.rs` and `scripts/exit/s416.sh` are in `PROTECTED`. The owner approved this on 2026-11-15 ("Yes, fix it here"). This tightens a gate and does not loosen one.

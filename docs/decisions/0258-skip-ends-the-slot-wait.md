# 0258: a skip ends a suite's wait for its slot

**Decision.** A suite's build and run slots are waited for with the suite's skip check as part of the broker's liveness test. A skip that arrives before or during the wait ends it, and the suite takes no slot.

**Why.** A skipped suite has nothing to run, so any slot it takes is a slot another task waits for. On 2026-11-13 #288 was skipped while in line for a compile slot and still took one, built, and only then noticed.

**Trust.** `jobs.rs`, `suites.rs` and `tests/testing.rs` are not in `PROTECTED`. No gate, limit or test is weakened; a skip still records its debt and downgrades the verdict.

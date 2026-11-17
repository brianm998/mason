# 0513: run_check names tests that are red on develop too

**Decision.** When `run_check` reports failing tests, each one that also failed in the newest stored baseline of that suite is marked "red on develop at <sha> too: not your change; merge develop when it is fixed."

**Why.** On 2026-11-16 a test red on develop was not quarantined, so a dozen agents spent rounds looking for a cause that was not theirs. Quarantine needs several tasks red and red at the base; the baselines already say what failed at develop.

**Trust.** Only `crates/daemon/src/jobs.rs`, `knownred.rs` and the store (not in `PROTECTED`). No verdict logic changed: this is text in the output only.

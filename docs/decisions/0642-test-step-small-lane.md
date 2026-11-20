# 0642: the test step's light runs take test-small

Status: proposed, waits for the owner (suites.rs is protected)

**Context.** Narrowing makes many test steps run a few executables in minutes, yet each queued for `test-suite` behind 15–30 minute runs while the CPU idled.

**Decision.** Record each test executable's own time from libtest's summary line. A narrowed suite whose selected targets all have a record, and whose medians sum to under `small_run_s` (default 180), takes `test-small` through the same `jobs::small_lane` rule as a scoped `run_check`. A target without a record counts as heavy, so the daemon's `it` (about 536 s) never qualifies; a whole run never does; without `[resource.test-small]` nothing changes. No limit is raised.

**Consequences.** Runs after deploy have no records and stay on `test-suite` until a run has been seen. Targets are keyed by executable name, so a name shared by two packages blends their medians; this can under-estimate.

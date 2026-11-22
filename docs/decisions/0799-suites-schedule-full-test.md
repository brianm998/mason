# 0799: A `suites` schedule runs MASON's own suites whole on the base tip as the full test

The `ci` schedule kind (0170, 0775) needs a GitHub workflow that runs everything. A project without one, a machine with remote runs off, or a suite CI does not run (MASON's `exit-scripts-landing`, a project's device suites) got no scheduled full test. Task #783 adds `kind = "suites"`.

**What it runs.** `suites = [...]` names suites of the definition; unnamed, it is every suite of the Test step plus the landing batch tier. They run one after another on a clean checkout of the base tip the decision read, each through `run_suite`: the same quarantine, the same immediate rerun of a red test, the same judge and the same executor choice as a debt payment (GitHub shards when remote is on, this Mac when off). A suite already recorded green at the tip is not run again unless the run is forced.

**When.** The same triggers, record and rule as 0775: `at`, `every`, `when_quiet`, `mason schedule run [--force]`, and `decide_full` (skip when the tip is fully tested or a test is running).

**Where it sits.** It is work below every task. It takes the Test step's resources with the lowest rank (`Rank::new(false, u32::MAX, u8::MAX, ..)`), so a task's request, however late, is granted first; with remote on those are `gha` pool units, not local slots. It also waits for the daemon's one-payment lock, so it never runs beside a debt payment.

**Judged like a payment, but not excused by the base.** The run's tip is the tree under test, so `trust_base` is off: a red is not excused as "also red at the base" (which on the tip itself would excuse every red). Quarantined tests are still set aside. A red test that fails again on the rerun is red; one that passes on the rerun is flaky.

**Recorded.** Each whole green is recorded at the commit (`suite_green`) and `settle_debts` closes every debt that green covers (#774). A red is put in `suite_red`. The verdict goes in `full_tests`: red if any suite is red, else green. A red suite files a `Normal` task with the failing tests and the range since the last green full test; a flaky one files a `Low` task; an open task with the same title suppresses a duplicate.

**Trust.** `crates/daemon/src/suites.rs` is in `PROTECTED` (verdict logic and recording of greens), so the change waits for the owner's approval. No gate, limit or test was weakened. `pay_with` is unchanged.

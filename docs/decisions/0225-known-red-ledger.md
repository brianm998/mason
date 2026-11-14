# 0225: one ledger of known-red tests

A test red at the merge base was excused per task and forgotten (a per-base cache), with no fix task on the suites path and no record on the command path.

**Ledger.** The `quarantine` table gains `source` (`owner` | `base` | `exhausted`), `fix_task`, `first_base`, `last_seen_ms`, `note`. Owner entries keep the owner + expiry rule (`check_quarantine`, at most 90 days). Ledger entries have no expiry (stored as the far future) because a proof lifts them. `quarantined()`, which feeds classification, returns owner entries only: a ledger entry never excuses a test on its own, the base run still decides.

**Add.** When a test fails at the base and is excused (`Baseline`, or `Product` with base-excused tests; or the command path's `excuse`), `record_red` adds an entry if there is none and files one urgent task via the existing dedupe (open task titled `<test> fails on`, or open tracker issue naming the test). Later sightings only move `last_seen_ms`. Names must pass `is_test_name`; source lines and prose are never added.

**Own step.** A task that is the `fix_task` of an entry gets no base excuse for that test: it is removed from the base list.

**Lift.** Only a run that executed the test and saw it pass: the ledger run after the fix lands (the fix's tests at the new tip, targeted by `rerun` when the suite has one, under a separate suite name so the whole suite's evidence and base cache are not touched) and a deferred debt run. A run that did not execute the test changes nothing. A red after landing keeps the entry, writes a note beside the fix task, and files a follow-up that links it.

**Visible.** `mason quarantine list` / `GET /quarantine` show source, fix task, age, last seen; a `known_red` attention item after 7 days when the fix is not moving.

**Why trust-listed files change:** verdict logic in `suites.rs` and the quarantine rule in `gates.rs` (the owner approves). No test, gate or limit is weakened: the fix task's own step is stricter.

**Not verified:** see docs/stages/s230.md.

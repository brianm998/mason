# 1047: Running work on other Macs ("nodes"): readings and choices

The plan is `docs/multi-machine-nodes-plan.md`; the owner approved it on 2026-11-23. This record
holds what building it settled where the plan left room, and what turned out different. Any item
can be overruled.

## Phase 0

- **Two questions in `decide`.** `may_leave` holds the rules every off-machine target shares (no
  `[remote_runs]`, `remote = false`, a leased device, `exclusive`, a failed setup check);
  `off_machine` chooses where. Only GitHub is a target so far. The old decision table passes
  unchanged, and `a_run_that_may_not_leave_stays_and_one_that_may_goes_off_machine` pins the split.
- **"Is it GitHub" became "is it off this machine" in three places** where the question was never
  about GitHub itself: the build-as-first-part-of-the-command rule (`suites.rs`), `suite_goes_remote`
  (a landing batch's hold) and the bisect's side-by-side check (`landing.rs`). The sites about
  GitHub's own pool (`gha_full`, the coverage map, `pool_needs`) stay GitHub-only.

## Registering one project on two coordinators is unsupported

A project has exactly one coordinator and so one landing queue; no landing lock crosses machines
(plan §3.13). `mason project add` warns when a paired node reports the same remote URL.

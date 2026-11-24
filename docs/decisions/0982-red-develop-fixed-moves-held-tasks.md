# 0982: A red develop that is fixed moves the tasks it held up

Stage s989. On 2026-11-22 twelve developing tasks failed the same develop-red tests in their
checks for 3 to 20 hours. Develop was fixed at 07:50 and nothing told an agent that the reds it was
chasing were gone, or brought the fix to a branch the base fold could not reach.

Decision:

- A baseline row can be partial (`baselines.partial`): the tests of a stale red baseline, run
  narrowed (`rerun::command`) at the new tip. `Store::latest_baseline` (what `known_header` reads)
  takes a test off the red list when a newer partial row passes it, and adds or re-bases a test a
  newer partial row fails. A partial row never replaces a whole row at the same base and is never
  read as a whole-suite cache (`baseline`, `ancestor_baselines`, `baseline_profile` skip it).
- `redwatch::pass`, a background loop (`red-watch`, one pass a minute at most), does the rerun by
  `hostedcheck` with `global: true` (the global placement switch decides where, as for a landing
  check). Tests that pass are "fixed". For each developing task whose last check failed one
  (`red_sightings`, written by `run_check`), develop is merged into its worktree with
  `fixsync::merge_clean` (the s525 rule: no uncommitted tracked change, no merge in progress,
  `git merge-tree` finds it clean) and a note is stored (`red_notes`) that `run_check` puts before
  its next result. A refused merge is told plainly (conflict files, or uncommitted work) and
  recorded in `red_fix_pending`; `redwatch::alerts` raises an owner alert when the task is still
  behind 30 minutes later, and clears it when the branch has the base.
- A test not known red and not in a current baseline that failed in the checks of three or more
  tasks within six hours is run alone at the tip. If it fails, a partial row records it (so every
  affected task's "red on develop" note now names it) and one urgent task is filed (deduplicated by
  the base-red gate's title). If it passes, the tasks are behind and are folded and told.
- Home's alert list says "N tasks held by a red develop (tests)" while a developing task's last
  check failed a test that is red on develop.
- A check waiting for slots when such a merge happens builds its spec again on the folded tree:
  `TestChecks::merges` adds the watch's folds (`red_folds`) to the session's own. A check that is
  already running on the old tree is stopped by its stop hook once the fold count changes and is
  made again on the folded tree, so the agent gets one result.
- The probe is placed as any run is and queued as any run is: a run bound for GitHub takes its
  runner units from the broker, ranked as owed work, before it is pushed; one that runs here takes
  the suite's slots the same way. It does not bypass the ranked queue.
- A developing task whose last check failed only tests that are red on develop shows the wait
  class `base-red` in the task view and the sidebar, with the test names and the fix task. It is a
  view over the sightings, not a recorded wait, so it does not enter the wait totals.

Why it is safe: no test is skipped, excused or weakened; a develop-red test still runs in every
check and still fails it. Only what the agent is told and when its branch catches up change. The
merge is the one `run_check` already does (s525, `basefold`), under the repository lock, and it
leaves a dirty or conflicting tree alone. `suites.rs` and `suites/base_red.rs` (protected) are not
touched. `jobs.rs` (check path), `crates/store` and `attention.rs` change what an agent or the
owner is told about a red; `knownred.rs` and `basefold.rs` are unchanged.

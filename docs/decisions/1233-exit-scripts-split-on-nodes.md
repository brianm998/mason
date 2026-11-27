# 1233: exit-scripts split on nodes, passes kept by the coordinator

## Problem
Only the GitHub path split a sharded suite. `node_check` sent the whole command as one job, so an agent's `exit-scripts` check ran its scripts one after another on a node and ended at its 90 minute limit with scripts `not run: out of time`; a rerun began again at the first script because a node has neither `MASON_TASK_CACHE` nor the tree.

## Decision
1. **Split.** `node_check` makes one node job per shard of a suite (`shards > 1`, the check being the suite's own command) with the suite's shard command (the narrowed `dev_shard_command` when it has no `{}`), `MASON_SHARD=i`, `MASON_SHARDS=n` and `.mason/run.json` naming only that shard. The results are merged as on GitHub (`shards::merge`: outputs in shard order, worst status). A job that times out makes the check red.
2. **Slots and leases.** The check's lease holds what it held before: one unit of the node's pool. The extra jobs are not extra leases; they use the node's own job slots, which the node enforces (409, no room) and which the coordinator reads from the node's status (the pool limit follows the same status). One check runs at most half the node's `jobs.max` at once (at least 1, at most its shards) and at most what is free when it starts. Shards beyond that start as its own jobs end, so every shard still runs exactly once, also with fewer free slots than shards. A job that finds no room waits only while another job of the same check is out; a node with no room for the first job gives the check back to the next place, as an unsplit run does. A stop or a lost job cancels the others.
3. **Passes kept.** For an agent's `exit-scripts` check (`MASON_EXIT_CACHE=1`, a `MASON_TASK_CACHE` directory) off this Mac, the coordinator sends `MASON_EXIT_TREE_KEY` (the check's `tree_key`) and `MASON_EXIT_PASSES` (keys kept under `<cache>/exit-scripts-pass-remote`). The runner keys a pass as before (script, content, `MASON_EXIT_REPEAT`, tree), skips listed ones as `(cached)` and prints `MASON-EXIT-KEY <key>` per pass; the coordinator stores the keys and removes those lines from the output. A test step and a landing run set no `MASON_EXIT_CACHE`: nothing is sent or kept. A changed tree (for example a merged, moved develop) changes the key, so nothing stale is reused. A fail is never kept. The same runs on GitHub use this too.
4. **Say when it cannot fit.** A result with `not run: out of time` gets a line: how many ran, how many were already passed, how many not run, minutes, how many ways it was split (and fewer than the suite asks, if so), and whether a rerun continues from kept passes or would start at the same place.

No timeout is raised and no script is dropped.

## Trust files
- `scripts/run-changed-exit-scripts.sh` is not in `scripts/exit/**`. It changes only how already-passed scripts are skipped, and only when the coordinator sends a key; the budget, the margin, the floor and the per-script limit are untouched.
- `scripts/exit/s1243.sh` is the new exit test. `.mason/**`, `suites.rs`, `sched.rs` and `crates/scheduler/**` are not changed (`suites::shard_lists` and `lists_partial` are only called).

## Not verified
The change was not run on the real node; the node tests use fake shard commands. A node whose status lacks `jobs.max` is treated as one slot.

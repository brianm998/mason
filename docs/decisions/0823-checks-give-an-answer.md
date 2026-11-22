# 0823: A check that gives no answer is retried by MASON, and big narrowed runs are sharded

Stage s830, ticket #873. On 2026-11-21 a develop agent's `run_check` on GitHub almost never answered:
a narrowed run went to one runner and hit its limit, `exit-scripts` ran whole on one runner, no
runner came or the push was refused, and each of these reached the agent as "FAILED", so it reran.

Decision, which the owner asked for:
1. A narrowed `cargo-workspace` check of the agent, of one target or several, is split across the
   suite's `shards` like the test step's (#880 did the exit scripts and one target). A recorded-time
   rule (`shard_over_s`) was not built.
2. A run that ended without a result about the code (no runner, refused push, runner limit with no
   failing test) is retried once on GitHub inside MASON, then goes to this machine where the
   project's fallback is local. After a second miss the agent gets "no result (infrastructure)",
   never "FAILED". No test is skipped or weakened: a sharded run judges the same tests and a
   retry replaces something that was not a result.
3. A skip of the task's testing ends its waiting and running checks and gives the same
   "Not run" answer as a later call.

Trust files touched with the owner's request: `suites.rs`, `scripts/exit/s830.sh`.

# 0842: the exit-scripts job gets 90 minutes

**Cause.** The full run (workflow_dispatch) leaves `MASON_EXIT_REPEAT` empty, so the repeat scripts (s170, s482, s497, s636, s773) use their default of 10 runs. The same ~94 s holds test runs 40 times, about 63 minutes of cargo time across shards. Shard 6 spent 49 minutes on other scripts before s636 started and needs about 16 more, so it ended near 65 minutes against a 60 minute job limit. The weights file also lacked s40, s61 and s84 (170-200 s each, default 60), so shard balancing under-counted them.

**Decision.** `timeout-minutes` of the exit-scripts job goes from 60 to 90. Weights for s40, s61, s84 are added. `MASON_EXIT_SCRIPT_LIMIT_S` (3600 full, 900 push) and `MASON_EXIT_REPEAT` are unchanged, so a hung script still fails by name.

**Rejected.**
- Cutting repeat counts on the full run: weakens a gate.
- Deleting s482 or s497: s668.sh pins them with expect counts, and they have their own task history.
- Raising the 3600 s per-script limit: it is not what was hit.

**Trust files.** `.github/workflows/**` and `scripts/exit/**` are protected; this change needs owner approval before landing.

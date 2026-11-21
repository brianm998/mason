# 0756: update s592's shard-selection assertion

**Context.** s592 guards the runner's `FAILED scripts:` summary and `exit $fail`. Its third check grepped for the old round-robin formula, which decision 0661 removed, so CI exit-scripts went red.

**Decision.** Change that check to the current weighted greedy selection (`tot[best] += wt[i]`, `owner[i] == si`). The runner is unchanged.

**Consequences.** No check is weakened: shard selection is still asserted, against the code that exists. s763 verifies it.

# 0623: nested runner tests must not inherit the CI shard

**Cause.** `MASON_EXIT_SHARD=4/6` set by CI leaked into the scratch runner in `scripts/exit/s602.sh`, which then selected none of its three fake scripts and exited 0 instead of 1.

**Decision.** Exit tests that run `scripts/run-exit-scripts.sh` on a scratch tree clear `MASON_EXIT_SHARD` first. The runner keeps reading the variable, since CI relies on it.

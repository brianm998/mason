# 0661: CI repeats less, limits each exit script, shards by weight

Status: accepted, pending owner approval (protected files: `.github/workflows/**`, `scripts/run-exit-scripts.sh`, `scripts/exit/**`).

**Context.** develop's CI did not finish for over a day: exit-script shards hit the 60-minute limit and `test` the 40-minute one, so failures were never seen green and confirmed.

**Decision.**
1. Repetition scripts read `MASON_EXIT_REPEAT`; CI sets 2 on push, the nightly run uses the scripts' own counts.
2. The runner kills a script over `MASON_EXIT_SCRIPT_LIMIT_S` and reports it by name.
3. Shards are assigned greedily by weight from a checked-in file, so adding a script moves no other.
4. `test` is split into `--exclude mason-daemon` and `-p mason-daemon` legs.

**This weakens no check.** Every script still runs on every push; only the repeat counts of stability scripts drop on push (full counts run nightly and locally), and a script over its limit now fails by name instead of the job being cancelled.

**Consequences.** Weights go stale; `scripts/refresh-exit-weights.sh` refreshes them from logs.

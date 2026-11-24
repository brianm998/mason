# 1029: exit-script ubuntu shard 2 red (s77, s117, s902)

## Decision

No code changes. `CI/exit-scripts (ubuntu-latest, 2)` failed twice on develop a0f67a2ed (run 45143216377) with `FAILED scripts: scripts/exit/s77.sh scripts/exit/s117.sh scripts/exit/s902.sh`. The log tail shows only that line; the per-script `FAILED:` lines and any panic sit above it. This session had no network, so the first panicked assertion or failing command for each script is not recorded: the log was not read and nothing was tried against it.

Each script exited 1; none printed `(over N s)`, so none timed out. Candidates from source only, unconfirmed: s77 has fixed sleeps and deadlines; s117 runs the adapters and daemon lib tests, which may be host-specific; s902 runs `a_check_wait_is_credited` and then s787.sh, which sleeps and uses `kill -9`.

No assertion is removed or loosened, and no sleep-based deadline is added. Settle with: `gh run view 45143216377 --log-failed | grep -nE 'panicked at|FAILED:|== scripts|FAILED scripts'`, then fix each by its recorded cause (host gate or hermetic test, fixed test, or waits on observed state).

# 0772: the test wait floor is an environment variable, set per remote run

Decision: `MASON_TEST_MIN_WAIT_S` (default 900) replaces the constant floor, and `[remote_runs] test_env` carries it to GitHub runners through `run.json`. Local behaviour is unchanged. Rejected: lowering the constant (a busy or syspolicyd-stalled Mac needs it), and a separate field in `run.json` (the existing `MASON_*` environment already reaches the runner). The `ci.yml` line is protected and waits for the owner.

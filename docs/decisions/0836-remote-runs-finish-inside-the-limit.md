# 0836: remote runs finish inside the limit by sharding and per-script deadlines

Status: proposed (touches `.mason/**`, `scripts/exit/**`, `crates/testing/**` and `suites.rs`, all protected; needs the owner's approval).

Remote cargo-workspace and exit-scripts runs were cut off by the 90-minute `run` step limit 117 times in a day, and a cut-off run has no verdict, so it is run again.

Decision:

- Shard exit-scripts four ways on GitHub, by the existing weighted plan of `scripts/run-exit-scripts.sh`, and hold each script to 900 s (`MASON_EXIT_SCRIPT_LIMIT_S`) so one hang is a red script, not a lost run.
- Add `dev_shard_command` so a narrowed cargo-workspace run of packages or one test target is split like the whole run. Narrowed picks the sharder cannot run (several groups, name filters) stay on one runner.
- Give `scripts/test-shard.py` an opt-in per-executable deadline, `MASON_TEST_EXE_LIMIT_S`.
- Make s521 run only the lint tests it names.

Rejected: raising `test_timeout_s`, which hides the cause and keeps the runner busy longer.

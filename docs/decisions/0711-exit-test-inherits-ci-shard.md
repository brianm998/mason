# 0711: an exit test must not inherit the CI job's shard

s668 failed on CI shard 5/6 because the job's MASON_EXIT_SHARD leaked into the runner it starts on fixtures. Decision: the test clears the MASON_EXIT_* selection variables and pins its own shard, rather than changing the runner, whose env contract is correct. s718.sh guards it.

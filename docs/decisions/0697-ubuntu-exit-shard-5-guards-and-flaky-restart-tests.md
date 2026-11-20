# 0697: ubuntu exit shard 5 guards and flaky restart tests

Context: shard 5 of the ubuntu exit-scripts job failed in s450 (check-exit-guards flags s645 and s662, which run swift and gradle without testing their skip variables) and s486 (live-token timing).

Choice: s645 and s662 get the same guard as s661; the cost wait in live_tokens.rs polls for the cost when one is expected. No test is removed, and no bound is raised.

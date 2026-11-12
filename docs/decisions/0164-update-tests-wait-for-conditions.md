# 0164: The update tests wait for conditions, not time

Status: accepted

The two flaky update tests guessed at timing (a 1 s sleep, a 3 s sleep, an 800 ms liveness guess) and could pick the explore step's host. They now wait for a marker the agent writes, the boot log's healthy line, and the daemon's pid in `daemon.json`. Waits keep generous limits that only stop a hang. The protected `update.rs` was not touched, because the boot log and `daemon.json` already carry what is needed. Races were identified by reading, not reproduced (see docs/stages/s169.md).

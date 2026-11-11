# 0114: the sweep ignores a host the daemon is already stopping

**Decision.** The daemon records each step host it asks to stop (`Daemon::stop_log`). Sweep step 3 does not file `orphan_host` for a Running host asked to stop within the last 30 s. Hosts never asked to stop are still confirmed on two passes.

**Why.** When a step closes, `StopStepHost` asks its hosts to stop, but the registry says Running until the process exits. A sweep seeing that host with a closed step run, on two passes, filed a false bug during normal progress (`a_healthy_daemon_files_no_bugs_across_many_passes`). The orphan case the sweep exists for is a stop that was never requested, which this does not hide; after 30 s a host still Running is flagged again.

**Not chosen.** Lengthening the confirmation to more passes: it only moves the race.

# 0238: monitor sessions do their slow work off the session loop

A session ran commands and view rebuilds inline in its `select!`, so one slow command or a busy view pool stopped pings and subscriptions, and a new task's view queued behind every client's full rebuild.

**Decision.** Commands go to a per-session worker (ordered results) and come back over a channel. Each rebuild is one spawned pool job, at most one per session, with a dirty flag for ticks meanwhile. A Subscribe sends the newly requested kinds first on their own job. The view pool is 4 threads. The hello/welcome/snapshot sequence and reconnect contract are unchanged. No protected file, gate or test is touched.

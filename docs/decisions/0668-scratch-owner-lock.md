# 0668: a scratch world's owner is the process holding its `owner.lock`

**Decision.** Each test scratch root holds an exclusive `flock` on `owner.lock`; the sweep treats a root as dead when that lock is free (with a pid written), and removes it with its hosts.

**Why.** The kernel drops a flock when the process dies, however it dies, so ownership needs no pid-reuse guesses and no timing. The pid is written after the lock so a root between create and lock (empty file) is never mistaken for dead. Roots without a lock may belong to an older test binary, so only those over a day old are removed. Test-only change; no gate, limit or trust file is touched. Removal happens off the test's critical path (a background thread, lock file last), and lock-less old roots are removed only when no process names them, never with processes stopped.

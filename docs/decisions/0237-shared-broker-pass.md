# 0237: one broker pass serves all waiters, and probes run outside the lock

**Decision.** A waiter in `Broker::acquire` runs `pass` only if no pass has run since its last wake-up. Lock-file probes (`Locks::alive`, `Locks::slots`) are done with `self.st` released and applied after re-taking it. Releases and submits still pass immediately. The `holds(id)` check and the wait share one hold of the lock, so a release cannot be missed.

**Why.** The old shape cost O(waiters) passes per tick with file I/O under the busiest mutex, starving monitor views and releases when the USB disk stalled.

**Trust.** `sched.rs` is not in `PROTECTED`; no gate, test or limit is weakened and ranking is untouched.

**Not verified:** behaviour under a real disk stall.

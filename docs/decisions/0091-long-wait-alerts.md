# 0091: Long waits and never-fits requests are alerts, worked out from the ledger

A wait is long when nothing has been granted on its resource for `[stall] long_wait` (default 60 min), measured from the later of the last grant and the request's arrival. This, not the age of the wait, is what separates a stuck resource from a long queue that is moving. One alert per resource (`long-wait:<resource>`, kind `long_wait`) rather than per task, so a pile of waiters is one item. A request the scheduler knows can never fit (`TooBig`, `Unknown`) alerts at once as `never-fits:<resource>`, with the limit and the command that fixes it; the long-wait setting does not govern it.

The ledger remembers last-grant times and answers `stalls(now, ..)` with the clock passed in; it is only read, so no grant is decided differently. Disk waits are excluded (existing alert); provider holds and pauses are not broker waits. The kernel-lock loop after a grant now says what it waits for after 60 s and counts as a long wait; `Io` errors are logged once per distinct message.

Trust: `sched.rs` and `crates/scheduler/**` are budgets and `attention.rs` is a trust file; the change only observes, and adds one line to `alerts_now`. Owner approval is needed for those files.

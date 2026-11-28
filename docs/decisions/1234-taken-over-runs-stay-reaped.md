# 1234: taken-over check runs stay with the reaper

Decision: keep taken-over runs in a broker list that the reaper reads, rather than cancelling
at takeover. Why: the judge already knows when a run is superseded, delivered or attached to by
a replayed call (#1051), so it will not cancel a run a replayed call is about to collect.
No gate, guard or budget changed; `sched.rs` is a protected file and needs owner approval.

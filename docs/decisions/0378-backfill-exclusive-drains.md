# 0378: backfill while the machine drains

**Decision.** A request that uses the machine may pass a waiting exclusive request only if it is sure to be done before the machine would have drained anyway. "Sure" means its estimate (the longest of its suite's last five times, plus a quarter, and only with two or more on record) fits before the latest expected end of the current machine-class holders. If any holder has no estimate, or the candidate has none, nothing passes, as before.

**Why.** Draining keeps an exclusive request from being starved, but it idles the machine for as long as the slowest holder runs. Work that finishes inside that window delays the exclusive request not at all, so letting it run is pure gain. Estimates come from what the same suite took before, recorded from the step host's own timestamps. A holder that overruns its estimate makes the drain's end lie in the past, and then nothing passes.

**Trust.** `crates/scheduler/**`, `crates/daemon/src/sched.rs`, `crates/daemon/src/suites.rs` and `scripts/exit/s385.sh` are in `PROTECTED`. The owner asked for this on 2026-11-14 ("Yes, I want you to do both, here, now"). Nothing is weakened: every request still gets exactly what it asks for, an exclusive request still runs alone, and its start is not delayed.

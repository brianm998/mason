# 0334: a step with an exclusive suite queues suite by suite

**Decision.** A test step queues suite by suite (decision 0173) when any selected suite needs `exclusive`, as well as when one builds first. Base-red checks stay tied to suites that build.

**Why.** A step-wide lease is the union of its suites' needs. With `exclusive` in it, one step keeps every build and test of every project off the machine for its whole run, though only its exclusive suites need that (2026-11-14: about 2 of 17 minutes, 26 builds waiting, CPU at 8%). Queuing suite by suite costs a wait between suites, which a step-wide lease avoided. That is cheap next to idling the machine.

**Trust.** `crates/daemon/src/suites.rs` and `scripts/exit/s340.sh` are in `PROTECTED`. The owner approved this change on 2026-11-14 ("Yes, fix it here"). Nothing is weakened: every suite still runs with what it asks for, and an exclusive suite still has the whole machine while it runs.

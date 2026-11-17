# 0545: a waiting exclusive request is bounded, and only a person gets the machine

Status: accepted for review (s552). Budget code (`crates/scheduler/**`, `sched.rs`).

## Decision

1. Backfill under a waiting exclusive request uses a drain end that follows overrunning
   holders (now + their expected run), never one in the past. Why: test runs vary 20 to 90 min
   and the old end was almost always past, so nothing passed.
2. An exclusive request that has waited `[order] exclusive_max_wait_s` (600) gives up its
   freeze; work with an estimate that is done by the drain end runs again. Why: a request
   waiting 13 min idled the machine twice. It keeps first place and starts when it drains.
3. A `mason run` is ranked as a person's only when its origin is a person at a terminal.
   Agent, hook, task and no-terminal requests are pipeline work. Why: an agent's ad hoc build
   took the top rank and strongest lock.
4. A non-person `--exclusive` becomes all `compile` units unless `[order] exclusive_origins`
   allows the origin. Why: one xcodebuild archive does not need the whole machine.

A person's exclusive request still gets the machine and the top rank.

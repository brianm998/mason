# 0674: a converted exclusive request keeps s552's bound and backfill

Decision: mark a request that `bound_exclusive` converted (`drains_compile`) and give it the
exclusive request's passing rules scoped to compile, reusing `drain_end` (compile holders only)
and `thawed`, instead of reserving all compile units, which froze compile with no limit.
Alternative rejected: leaving the reservation and bolting a timer on, a second copy of the rules.
A person's exclusive request and an `exclusive_origins` request take the unchanged path.
Budget code (`crates/scheduler`, `sched.rs`): owner approval needed.

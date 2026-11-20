# 0686: pin the submitted head in a landing batch

Status: accepted (pending owner approval: landing.rs and crates/landing are protected)

A batch merged each task's branch by name every round, so commits made after testing
could land. Decision: record the head at queue entry, merge that commit, drop members whose
run ended, and park a task whose head differs from the tested one. This only tightens what
may land.

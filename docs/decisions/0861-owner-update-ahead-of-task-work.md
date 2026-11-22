# 0861: the owner's update build goes ahead of task work and past the heat hold

Decision (s868, #923, owner-approved 2026-11-21): the self-update build is one `compile` slot with `CARGO_BUILD_JOBS = cpus/8`, so it is small. An update the owner starts gets the interactive rank and is exempt from the CPU, stall, syspolicyd, pacing and thermal holds. A die over `thermal_max_c + 2` still holds it. An automatic update keeps s473's lowest rank for `update_wait_max_s` (30 min), then is reopened with the owner's rank.

Why: a daemon that cannot update cannot take the fixes that would clear its own queue.

Trust: `update.rs` and `sched.rs` are protected; this changes only when the update's own build may start. No task gate changes.

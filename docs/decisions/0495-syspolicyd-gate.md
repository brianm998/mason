# 0495: further compiles wait while syspolicyd is overloaded

**Decision.** The `compile` gate also reads macOS's `syspolicyd` CPU use. A further compile, one started beside a compile already running, waits while that reading is at or above `[cpu] syspolicyd_max_pct` (default 300%, 0 off). The first compile always starts, and agent sessions are never held.

**Why.** `syspolicyd` scans each new executable the first time it runs, whatever its signature, at a cost that grows with the file's size. MASON's builds produce hundreds of large new test binaries per task. On 2026-11-16 that load kept `syspolicyd` at about 7 cores until WindowServer's watchdog killed the owner's session (s502). Signing every binary would not have helped: it was measured, and it made no difference.

**Why not the CPU gate.** That gate reads the whole machine. The scans run in Apple's process, and the compiles that cause them mostly wait on it, so CPU use understates the harm, much as a disk stall shows as idle CPU.

**Trust.** `crates/scheduler/**` and `crates/daemon/src/sched.rs` are in `PROTECTED` (budgets). The owner approved this on 2026-11-16 ("Yes, add the gate").

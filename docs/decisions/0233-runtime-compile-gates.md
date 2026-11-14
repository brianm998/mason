# 0233: compile gates are data the owner changes at runtime, kept in gates.kv

**Decision.** The seven compile gates (`compile_cpu_max`, `compile_settle_s`, `compile_jobs`, `compile_idle_kill_s`, `stall_ms`, `memory.floor_mb`, `disk.floor_gb`) can be changed while masond runs, by the owner only, and are kept in `gates.kv` next to `limits.kv`, read after `resources.toml`.

**Why.** A restart kills every step host (#267), so editing `resources.toml` was not a way to tune a busy queue. Limits already worked this way (`set_limit`, `limits.kv`); the gates next to them did not.

**Choices.**
- One file and one set of bounds: `check_gate` repeats `parse_resource_cfg`'s bounds (1–100 for the CPU gate) and only tightens (u32 gates refuse values above u32). The default 50 is untouched.
- A reset removes the override and returns to the file's value, or the default; `mason resources` says which of `default`, `resources.toml` and `runtime` a value is.
- `compile_jobs` set at runtime does not override a `compile` limit the owner set (`limits.kv` or `[machine]`): the count stays the owner's word.
- Over HTTP the reset is `value: null` (no DELETE route), which is what the WS `SetGate.reset` sends.
- The monitors show only the CPU gate and the settle time; the rest are CLI and API.

**Trust files.** `sched.rs` and `clients/features.toml` change; this widens who can adjust an existing gate (the owner, live), not what a gate is.

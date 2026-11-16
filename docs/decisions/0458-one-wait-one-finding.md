# 0458: One wait is one finding

One slow CPU-gated wait raised `overloaded:slots`, `overloaded:cpu-gate` and `overloaded:load` together. Now:

- When a wait listed under `overloaded:slots` has a CPU cause (`health::is_cpu_cause`, shared with the daemon), its evidence line says "held by the CPU gate" and `overloaded:cpu-gate` is not raised. It fires only when no listed wait has a CPU cause.
- `overloaded:load` is raised only for a measured reading. `Facts::load_assumed` is set by the daemon when the sampler's source is `assumed-busy`.
- All keys and summaries are unchanged, so cooldowns keep working. Judgement stays pure in core.

Note: at defaults `slot_wait_ms` and `cpu_gate_ms` are both 30 min and slots uses `>` while cpu-gate uses `>=`, so cpu-gate is nearly dormant at defaults; it matters when the owner sets `cpu_gate_m` below the slot wait. The owner can drop it later.

Rejected: a single merged `overloaded` finding (would change keys and cooldowns).

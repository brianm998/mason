# 0693: the thermal gate reads the SMC die temperature and power limiting

Status: proposed (adds the `thermal_max_c` limit in `sched.rs`, a protected file; waits for the owner's approval)

**Decision.** The gate is hot when `pmset`'s speed limit is under `thermal_min_pct`, or the CPU die is at or above `thermal_max_c` (default 97 °C), or the SMC has power-limited the package for 30 s or longer. The `HOLD_MS` hold applies to all three. Readings come from `AppleSMC` through IOKit without root; when they are missing the gate is exactly the old one.

**Why.** On 2026-11-18/19 the die was at 90 °C or more for 647 minutes and power-limited for 306 while `CPU_Speed_Limit` read 100, so the gate never fired.

**Kept.** `ThermalGate`, the ledger and the "no hold with nothing running" rule are unchanged; running work is never revoked. `thermal_max_c = 0` turns the new checks off.

**Cost.** Raw FFI to IOKit (`unsafe`, allowed in two small functions with SAFETY notes). The power-limit key is unconfirmed (see s700).

# 0507: the syspolicyd gate is shown in the monitors and alerts after 30 minutes

**Decision.** The syspolicyd reading and its limit travel in the Compilation view and the resources snapshot. Both monitors show them. The owner is alerted when the reading has stayed at or over the limit for 30 minutes without a break.

**Why.** The gate (s502, 0495) was visible only in `mason resources` and in a held task's wait text. A hold that lasts half an hour means something besides MASON is swamping `syspolicyd`, which MASON cannot fix and the owner can.

**Why 30 minutes.** It matches the other long-hold thresholds (the CPU gate's 30 min in the health check), and a build's burst of first launches clears well within it. It is a judgment, not a measurement.

**Trust.** `crates/daemon/src/sched.rs` and `clients/features.toml` are in `PROTECTED`; this waits for the owner's approval. No limit or gate is changed: the gate's behaviour is untouched, only reported.

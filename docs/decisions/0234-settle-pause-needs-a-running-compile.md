# 0234: the compile pacing pause applies only while a compile is running

The gap between compile starts (`compile_settle_s`, 30 s) lets a started compile show in the CPU reading before the CPU gate admits another. With no compile running there is nothing to show, and the CPU gate already lets the first compile start. The pause now follows the same rule: `settling` requires a compile running in the plan's usage (held, or granted earlier in the same pass), so later compiles in a pass are still paced one per gap.

Wait text: a request waiting behind an earlier waiter with no holder names that waiter and its reason ("behind #289 (letting the last compile show in CPU use, 7 s)") instead of reading as a full slot.

**Why a trust file changes.** `crates/scheduler/**` is protected (budgets). The change narrows when an existing pause applies, to the case it was made for. The CPU gate and its limit, the pacing gap, the compile count, and the disk, stall and memory gates are unchanged.

**Not verified:** behaviour on a live daemon under load; compiles per minute with short builds.

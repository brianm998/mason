# 0150: Another pipeline queues in masond

Two pipelines on one Mac kept two queues. Stage 155 lets an outside pipeline queue with masond, off unless a caller sends the new fields.

**Rank.** A request with `pipeline` is `Rank::new(false, steps, priority, now, seq, false)`: pipeline work, never interactive, so a person's `mason run` still sorts first. The existing order applies (steps, then priority, then arrival; `[order] priority_first` swaps the first two). `crates/scheduler` is not edited; the only scheduler file touched is `sched.rs` in the daemon (a protected file, listed in the submission). Ordering against a MASON task at equal steps remaining: outside urgent before any priority; outside high before normal and low; outside normal before low and after high; outside low after all. The ticket's "normal waits behind a MASON high step" holds only at equal steps (or with `priority_first`); by default fewer steps remaining win whatever the priority.

**Progress.** `progress` 0 to 6 maps to `6 - progress` steps remaining, so the default (no progress) is 6, the farthest from landing, and the least likely to jump a MASON task. Progress 6 is zero steps. Larger values are refused with a 400.

**Priority names** map urgent 0, high 1, normal 2, low 3. An unknown name is refused.

**Label.** The daemon shows `NAME LABEL` (`starpipe #483 develop`). The CLI does the same for the label in the kernel slot files. A waiter reads `LABEL: waiting for RES (N ahead)`, where N is the waiters ahead in line order and RES is the first resource it asked for.

**Hold.** `mason hold` shares acquisition with `mason run`. It prints `granted` after the kernel locks are taken, then ends on stdin EOF or when `--pid` is gone (polled every 150 ms with `kill(pid, 0)`; EPERM counts as alive). It installs no signal handler: SIGTERM and SIGKILL end the process, the kernel frees the locks, and the daemon reaps the lease (the same liveness `mason run` has). `llm:*` is accounting only, so for it the lease reaping is what frees the slot.

**One count.** An outside hold uses the same ledger resource `llm:claude`, so `[llm] claude` limits both and admission of MASON tasks (`llm_free`) sees it.

**Monitors.** The Swift and Compose monitors are not changed: showing outside work needs a protocol schema change and a `clients/features.toml` entry, both protected. The JSON carries `pipeline` for them to read later.

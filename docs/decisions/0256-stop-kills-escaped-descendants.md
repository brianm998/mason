# 0256: stopping a step kills the descendants noted beforehand, not just its process group

**Decision.** `stop_child` takes a snapshot of the child's tree (by parent pid and process-group membership, with each pid's start time) before it signals anything. It then signals the group and each snapshot member, kills what survives the grace period, and sweeps the snapshot once more after the child exits. A pid is signalled only if it still has the start time it had in the snapshot; pids 0 and 1 and the step host are never in it.

**Why.** Test runners and tool wrappers move workers to their own process groups. Those escape the group signal, and after their parent dies they cannot be found by ppid, so the snapshot has to come first. On 2026-11-13 five xctest workers ran for minutes after a skipped step.

**Alternatives.** Walking the tree only after the child dies finds nothing (reparented to launchd). Running every step in a Linux-style cgroup or macOS job has no macOS equivalent that does not need privileges. Killing by process name would hit other sessions' processes.

**Limits.** A process that escapes and is started in the window between the last snapshot and the child's death is not found. Windows keeps its current behaviour.

**Trust.** `crates/step-host` and `crates/platform` are not in `PROTECTED`; `scripts/exit/s261.sh` (exit tests) is. The owner asked for this fix on 2026-11-13 ("Yes, fix it"). No gate, limit or test is weakened. (Corrected in s271: the first version said none of these paths was protected.)

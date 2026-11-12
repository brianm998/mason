<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0147: who watches a silent tree, and what ships on

Context: starpipe wakes stopped processes with SIGCONT after silence and kills a tree that has been
silent with no progress; MASON had neither for suites, command steps or `mason run` children (task
#166, `docs/decisions/0119-star-migration-inventory.md`). The owner wants no logic running in two
places, so everything ships off.

Decisions

1. **The watcher lives in the daemon's poll loops, not in the step host.** A suite (`run_hosted_once`)
   and a command step (`run_command_on`) start a `watch::Watch` thread beside their existing limit
   thread; it reads the child pid from the host's own record, `ps` once a tick, and decides with the
   pure `watch::tick`. The step-host protocol does not change. A daemon restart that re-attaches
   starts a new watcher, and its silence clock starts over (a restart never kills for old silence).
   `mason run` children run in no host, so the CLI runs the same `tick` on its own child's process
   group, with the thresholds the daemon returns when the lease is opened.
2. **Both keys default to 0 (off) in code**, `cont_after_s` and `kill_after_s` per project. The
   ticket says "defaults 10 and 30 minutes" and also "ships off by default"; the second wins. The 10
   and 30 minute values are written into star's overlay by the paired star ticket, in the same change
   that turns starpipe's own watcher off. A person's `mason run` (no `MASON_TASK`) is never watched.
3. **Silent** means no new bytes in the step's log and no change in the summed CPU time of the tree
   (the process and everything under it, plus its process group). A process ending lowers the sum and
   counts as movement. SIGCONT resets nothing; a tree that is re-stopped is continued again after
   another `cont_after`. Every stopped member is continued, the root included (a root left in `T`
   would hold the slot to the time limit). The `mason run` watcher sees CPU only: the child's output
   goes straight to the terminal.
4. **A tree with a waiter is never killed.** `POST /leases` now carries the requester's pid; the broker
   keeps it and `waiting_pids()` lists the requests still waiting. A tree holding one is spared (and
   still continued if stopped). `mason hold` has no separate waiting state in this tree, so only
   leases are covered.
5. **One watcher to a tree.** `hung.rs` stops a task's command after `compile_idle_kill_s` of no CPU.
   It now leaves a host alone while the stall watcher has `kill_after_s` set on it (`Daemon.watched`),
   and still shows the idleness. Agent steps keep their own idle handling and get no stall watcher.
6. **A kill is a red with its reason.** The watcher marks the run, stops the host through its own
   socket, and `judge_ran` turns it into `Verdict::Red` with `stalled: no output or CPU for N min`
   (seconds under a minute) rather than "ran out of time"; a command step is red with the same text and
   no rerun. A SIGCONT is noted in the step's progress message and both counts show in `GET /health`
   and `mason health` (in memory: they restart at 0 with the daemon).

Protected paths: `crates/daemon/src/suites.rs` (verdict logic: a stalled suite becomes red) and
`scripts/exit/s152.sh` (a gate). Both need the owner's approval.

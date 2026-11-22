# 0811: favor-local spills to GitHub when this machine is overloaded

Stage s818, task #832. Nothing measured "overloaded" when a run was placed: local runs queued for
`compile`/`test-suite` behind the CPU and thermal holds however long the queue got.

Decision: `favor-local` runs here unless the machine is overloaded (a hold is active; the slots the run
needs are all taken and others already wait; or the oldest local wait exceeds `[remote_runs]
spill_after`, default 120 s) **and** GitHub has a free `gha` unit. If both are full the run stays and
queues here: local is favoured. The reading is `LocalLoad`, built from a new read-only
`Broker::load_reading` (sched.rs is protected; no gate, limit or hold changes). A placement stands for
the run, and a run placed here that has waited past `spill_after` under a hold moves to GitHub at its
next placement. The owner asked for this on 2026-11-21.

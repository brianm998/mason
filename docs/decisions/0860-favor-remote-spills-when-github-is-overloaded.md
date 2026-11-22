# 0860: favor-remote spills to this machine when GitHub is overloaded

Stage s867, task #833. `favor-remote` always went to GitHub; the only spill was s812's fallback after
the whole `pickup_timeout` (default 1200 s) had been spent waiting, holding a `gha` unit and a run ref.

Decision: `favor-remote` runs on GitHub unless GitHub is overloaded **and** this machine has room.
GitHub is overloaded (`RemoteLoad::overloaded`) when the `gha` pool has no free unit and others wait,
the account has more than `[remote_runs] remote_queue_max` (default 4) jobs queued, or the median of
the last five pickup times is over `spill_after` (default 120 s). Room (`LocalLoad::has_room`) is no
hold and a needed slot free. Otherwise it goes to GitHub and waits there. Each placement says why.
A run already on GitHub and unpicked for `spill_after` is cancelled (ref deleted, units freed) and
runs here when there is room, before the pickup timeout; the timeout fallback stays as the safety net
under `favor-remote` only. Plain `remote` never spills: `Site::fallback_local` is now the effective
mode (task, project, global) being `favor-remote`, not the old `fallback = "local"` flag alone.

No gate or limit changes; `sched.rs` is only read.

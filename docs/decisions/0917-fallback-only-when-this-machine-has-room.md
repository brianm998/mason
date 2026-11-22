# 0917: A run GitHub has not picked up comes back only when this machine has room

Stage s924. With `fallback = "local"` (mode `favor-remote`), a run GitHub had not picked up within
`pickup_timeout` was cancelled and sent back here whatever this machine was doing. On 2026-11-22,
after a restart, about 16 runs were pushed at once, GitHub did not pick them up in 300 s, and all of
them came back to a Mac that was heat-held (CPU held to 41%): they waited 50+ minutes in the local
queue while GitHub ran 2 of its 20 jobs.

Decision: when the site can say whether this machine has room (`Site.local_room`, built by
`remote::site_for_run`), the pickup timeout no longer cancels the run unless there is room. The run
stays on GitHub, where a runner may still take it, and comes back the first time the timeout has
passed and there is room. Room is `place::no_room_for`: no CPU or thermal hold is active, and each
slot the fallback lease would ask for (`uses`, as `remote::fallback_lease` reads them) has a free
unit. It is read from `Broker::load_reading`, and asked at most every 2 s per run.

- One room function serves both the pickup timeout and favor-remote's early return (`spill_after`),
  so the two cannot disagree: the spill is the same check made earlier. Previously the spill's room
  test required only one of compile or the suite's slots free; it now needs each slot the fallback
  would hold, because that is what decides whether the lease is granted at once.
- A covered run (s911/0904: a landing batch holds the slots for it) is not counted against its own
  hold: only an active hold blocks it.
- A site without a room function (every other caller of `site_for`) behaves as before: back at the
  timeout.
- The two GitHub-failure branches (GitHub unreachable for `pickup_timeout`; a run that ended in
  `startup_failure` or without a result) are left alone. There is no queued run to wait for: the
  run is lost on GitHub either way, and holding it back would only delay a failure the caller
  already handles with the same lease.
- One INFO line per run, when a due fallback is first held back, names the hold or full slot.

Not decided here: an unbounded wait. A run with no room and no runner waits as long as both last;
the run's own limit does not start until a runner takes it. `exclusive` in `uses` is not part of the
room check.

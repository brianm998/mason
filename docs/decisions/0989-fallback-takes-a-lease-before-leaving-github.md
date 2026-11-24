# 0989: The local fallback takes its lease before it gives up GitHub

Stage s996, task #1025. On 2026-11-22 nineteen runs read "room" at the pickup timeout from one
instantaneous reading, cancelled their GitHub runs, and queued behind 18 requests on a gate that
started nothing.

Decision:

- Room is a grant now. `LocalLoad::no_room` treats a slot as blocked when its free units do not
  exceed the requests waiting for any needed slot. The count is all waiting requests for those
  slots (the reading carries no ranks), which can only err toward staying on GitHub.
- A hold stays active until clear for `compile_settle_s` (`sched::settled_hold`). The broker
  records when a hold was last seen on every gate evaluation, so a run that first looks after
  the hold dipped still sees it. `load_reading` is shared with `favor-local`, which therefore
  also sees the settled hold.
- The room function in `site_for_run` calls `Broker::try_acquire_beside` (a non-queueing grant,
  decided under the broker lock, so two runs cannot take the last unit) and keeps the lease in a
  registry that `fallback_lease` and `fallback_lease_for_batch` read. Only then is the GitHub run
  cancelled. A lease nobody collects is dropped after 10 minutes. No change in `crates/scheduler`.
- `acquire_as` returns `Cancelled` when its request is no longer in the ledger.
- The pickup clock starts at `created_at` when that lies between the push and now.

- An exclusive request waiting in the broker (`LoadReading::exclusive_waiting`) is no room for
  `no_room`, `has_room` and `overloaded`, so favor-remote and favor-local do not place work into
  the convoy behind it (seen 2026-11-23 with a whole-machine test step).

Not done: step 3, moving a queued fallback back to GitHub. With the lease taken first, a run
that falls back after the room check holds its lease already and never waits in
`fallback_lease`; only runs with no room function (GitHub down, no `[remote_runs]` spill) queue
there, and they have no GitHub run left to go back to. A re-send would need `remote::execute`
to be re-entered from `fallback_lease`'s callers. Left as a follow-up if a queued fallback is
ever seen again.

`favor-remote` should be safe again once this lands, but that rests on unit tests with fake
readings, not on a real daemon run. The owner's `mode = "remote"` setting is untouched.

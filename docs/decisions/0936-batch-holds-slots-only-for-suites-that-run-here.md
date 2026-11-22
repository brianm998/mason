# 0936: A landing batch holds this machine's slots only for the suites that run on this machine

Stage s943. A landing batch took the `compile` and `test-suite` slots of every suite it was about
to run (`Runner::hold`), also for a suite that `suites::run_hosted_resuming` then placed on GitHub.
On 2026-11-22 a batch's `cargo-workspace` run was out on GitHub for 48+ minutes and held one of the
four `test-suite` slots all that time. Another project's suites use `exclusive`, and an exclusive
request waits for the whole machine to drain, so it waited out the batch's GitHub run and
everything queued behind it waited too. A project without remote runs lost a quarter of its test
capacity whenever a batch of another project was out. The same hold was half of the deadlock of
s911/0904.

Decision:

- Before it takes its slots, a batch asks of each suite what the run will ask:
  `remote::suite_goes_remote(d, project, None, suite)`, which is `executor_for(.., Kind::Suite,
  Some((None, suite)))`, the call `run_hosted_resuming` makes for a batch's suite when it starts
  (a batch has no task, so `None`). The lease holds `suites_needs` of the suites that stay here.
  A suite bound for GitHub takes nothing from this machine; it holds `gha` units through the
  remote module as before.
- Placement is not always known up front (`favor-local` and `favor-remote` read live load, and
  `place::place_of` keeps its answer for a short while so the two asks agree). The safe rule is
  made by the code that runs, not by the guess: whatever the batch's lease does not hold, a run
  takes when it needs it. A suite the batch took no slots for that runs here after all (placement
  changed, or a baseline or a build, which never go to GitHub) takes its slots at that moment as
  the batch: `remote::batch_lease`, ranked as the batch's first task (`d.rank_for(first, def,
  step)`, as `Runner::hold` ranks the lease), beside the lease. Slots the suite needs that the
  lease holds are not asked for again.
- A suite that was out and falls back here (`fallback = "local"`, s924's room-gated path) takes
  the slots of the definition's test step as the batch (`remote::fallback_lease_for_batch`), ranked
  as the batch's first task, not behind everything as a run with no task is (`fallback_rank` with
  no task).
- s911's `covered` is now per suite and true only when the batch's lease holds that suite's slots
  (`BatchCover::holds`: the lease exists and each amount the suite needs is within it). A covered
  fallback still takes nothing, which is what keeps the cycle of 0904 impossible; an uncovered one
  takes its own lease, so no run is both unleased and uncovered, and none is leased twice. In a
  batch with a local and a remote suite, the remote one is covered when its needs are within the
  local suites' (the lease is the largest of each amount).

Left alone: the checks of a landing batch (rebuild, re-test, fast tier, new tests). They have empty
`holds`, so a check that falls back from GitHub takes no lease of its own (s911) and runs under the
batch's hold; their holds stay as they were. The lint and floor holds are unchanged. The wait for
a lease taken as the batch in the mixed case can still queue behind an exclusive request for the
part the lease does not hold; the part it does hold is never asked for twice.

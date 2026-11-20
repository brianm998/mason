# 0660: the CI watch sees a job that hit its time limit

Status: accepted (s667).

**Problem.** GitHub gives a job cut off at `timeout-minutes` the job conclusion `cancelled`,
the same as a job of a run a newer push superseded. The watch ignored every conclusion but
success/failure, so a shard that timed out in every run stayed "waiting for a newer run" for
ever and the timeout was never filed.

**Detection (one, chosen).** A job `cancelled` inside a run that finished `failure` or
`success` whose check-run annotations contain "exceeded the maximum execution time" is given
the conclusion `timed_out` (`job_conclusion` in the daemon's `ci_watch.rs`). The task text
offered duration-versus-`timeout-minutes` as the alternative. I did not use it: the Actions
jobs API does not carry the job's limit, so it would need a second copy of ci.yml's limits
that drifts. The annotation is GitHub's own statement and gives the limit text for the ticket.
It costs one extra call, made only for a `cancelled` job in a finished run. A run that is
itself `cancelled` is never read this way (it is a superseded run; `cancel-in-progress` is off
for `develop`). The daemon test therefore feeds annotation text rather than a duration.

**Rules.** A `timed_out` job records `timeouts` (distinct runs) and `last_timeout`; it does
not change `red`, the red ticket or coverage. `agreement` runs with no success or failure
between file once, titled `CI/<job> exceeds its time limit on <base>`, body: run, commit, the
limit, the log tail and the `== scripts/exit/` lines of the log. A success or failure clears
the count and lets it file again. An open task naming the job suppresses it as for any other
filing. The status shows `timeouts` and the held line says
"…#T landed 3h ago; the 5 runs since timed out before finishing it". "Waiting for a newer
run" stays only when no run since has timed out.

**Not verified.** The real GitHub annotation endpoint (`/check-runs/{id}/annotations`) and
the exact annotation wording were taken from the ticket, not called. Whether the job log of a
timed-out job is readable was not checked. No gate, freeze or landing rule changed.

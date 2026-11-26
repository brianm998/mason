# 1077: cancel the CI runs MASON stops waiting on

## Context
A landing or debt CI wait that ended without a verdict deleted the branch but left the GitHub Actions run going until the 6-hour job limit. On 2026-11-24 several such runs held Windows runners for 4-8 hours on a hung test.

## Decision
The build adapter gets `cancel(run, git_ref)`. MASON calls it, best effort, before deleting a candidate branch whose run has no verdict. GitHub filters by branch as well as commit, because another `mason/ci/*` ref can test the same commit. `remove_refs` has no commit on the ref row, so it lists by branch; no schema change. On a `mason/pr/**` branch the pull request's own runs are cancelled: the branch is MASON's and is being closed. `may_cancel` and `cancel_own_run` are unchanged, so the early-red cancel and remote runs behave as before.

## Alternative
Storing the commit on the ref row: a migration for a lookup the branch name already answers. Widening `cancel_own_run` to `mason/pr/**`: it would also widen what its other callers may cancel.

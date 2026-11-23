# 0921: CI watch keys jobs by the workflow's name

Stage s928. A run's `name` is its display title when the workflow sets `run-name:`, so CI-watch keys
were "<commit message>/windows", different on every commit, and showed as whole messages in the monitor.

Decision: the adapter takes the workflow's own name (by `workflow_id`, else `path`, from the
workflow list, cached). The display title is not carried.

Saved state: entries keyed by a title are **dropped and rebuilt** from the next runs, not re-keyed.
An entry is dropped when its name has a line break, or when its `last_run` is a run in the current
list that now has a different workflow name. Re-keying was rejected: a title key says nothing
reliable about which workflow it was. Cost: a dropped entry's red/known flags are re-seeded, and an
open filed task is not linked to the new key until the job is red again; entries whose last run has
left the 100-run window and whose title is one line are kept and go stale harmlessly.

# 0387: split a red batch to find who breaks it

Status: accepted (owner approved the review item).

**Context.** When blame by impact named no one, or named everyone, and the bisection rung found no single task, every task in the batch went back to development ("batch #N is red in static checks and the red cannot be pinned on one task"): 20 times to star tasks in four days.

**Decision.** `plan::pin` searches a batch known to be red. It runs the suites over each half (the integration branch rebuilt from that subset) and descends into every half that is red. If neither half is red, the red needs several tasks: the set is shrunk one task at a time to those needed, and they are sent back together, each told the others' names. If every task reproduces the red alone, it is the base's: nobody is blamed, nothing lands, and the owner is alerted (`redbase`), as for a base that does not build. Otherwise only the tasks found go back, each with the output of the run that showed it, and the rest are verified again and land. The same search replaces both places that sent the whole batch back (the local CI-mapped search and the `bisect` rung). The progress message of each member names the subset being run; the batch's closing note says who reproduced it alone and who only together.

**Consequences.** More suite runs on a red batch that blame cannot pin (about 2 per level, plus one per task when shrinking an interaction). Two independent culprits in a two-task batch look like a base red, because each is red alone; that is the definition used. The `ask` rung is reached only for a CI-judged real failure or a ladder that has no `bisect`.

**Trust.** `crates/daemon/src/landing.rs` and `crates/landing/**` are in `PROTECTED`: the change is the red ladder's verdict logic, so it waits for the owner.

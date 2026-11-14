# 0180: Suite commands are told what they hold

A test step listing suites held a lease, but its suite and build commands got no `MASON_HELD`, so a command queueing for the same class waited behind its own step.

**Decision.** `suites::run` gives each suite and build command the step's `MASON_*` variables with `MASON_HELD` set to the classes of the lease held for that command (`exclusive` named when the lease is). `command_job` and `run_check` are unchanged.

**Trust.** No protected file is edited.

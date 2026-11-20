# 0659: run_check replies match the narrowed test step

Since decision 0539 the test step narrows `cargo-workspace` as `run_check` does, but the replies
still said it runs the whole workspace, so develop agents ran `full: true` (about 38 minutes each).

Decision: reword the replies and tool descriptions to say the test step runs what the change
affects and the landing batch runs everything. `full: true` still runs and is never refused or
limited; an optional `why` is logged with the check and echoed ("Full run (why: ...)"); without it
the reply asks for one next time. Visibility, not a gate. Suites without `dev_command` are
unchanged.

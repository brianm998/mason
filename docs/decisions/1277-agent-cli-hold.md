# 1277: An agent CLI that cannot start is a project hold, not a task failure

**Decision.** masond keeps, per project and in memory, whether its agent binary can be run. A held project claims nothing and launches nothing, and says so once. The hold is not a pause row and does not go through `sched.rs`: admission filters on it, as it does on the check-queue hold.

**A claimed step waits inside its job.** Nothing is emitted that the engine counts. The job drops its context and builds a new one when the hold lifts, so a corrected `claude_bin` or `agent_env` is what launches. The retry note it had read is put back.

**A failed launch is remembered against the file.** A file can pass every check and still not start. Looking at it again must not lift that hold, or the step would loop; a reload, or a change to the file, does. The memory is lost on a restart, which then tries one launch again.

**Only pipelines with an agent step are held**, so a project of commands alone needs no agent CLI. `mason doctor` warns rather than fails for those.

**The match is on the step host's message**, exit status 127 with `step-host: failed to start` first in the output. An agent that itself exits 127 is not taken for it.

**Trust file.** `attention.rs` changes in two places: the new alerts are listed, and tasks waiting for their agent are left out of the "pipeline has stopped moving" alert, which would otherwise be a second notice for the same fault. Nothing about who may answer changes.

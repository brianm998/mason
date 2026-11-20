# 0720: where a task runs is a task override, set only by the owner

Status: accepted (#699, s727)

**Decision.** The choice is the per-task override `run_on` = `github` | `local`. A task without it follows the global switch.

**Why an override.** Overrides already exist per task, survive restarts and are read live; `Task` gets no field, the log no event, the store no migration.

**Why only the owner.** The key moves work off this machine, so an imported issue or an agent's filing must not choose it: `filing::refuse_run_on_claim` refuses it, `add_task` and `task run-on` are the only writers, and the plain override routes refuse the key.

**Why it does not weaken a gate.** The place changes, the command, the parser and the minimum do not. A suite's own `remote = false`, a device or `exclusive` still keeps a run here.

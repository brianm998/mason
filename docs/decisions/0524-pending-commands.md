# 0524: monitors record a task command as pending before sending it

**Decision.** In both monitors the model chooses every task command's id (`act:<action>:<task>:<n>`) and records a pending entry before the command is written. The entry drives the pressed control's label and disables the task's other controls; it ends on a failure (error shown beside the control), or after masond accepted it and a later view shows the task changed, or after three such views.

**Why.** Nothing on screen changed between a click and masond's rebuilt view, which can take seconds, so the owner clicked again. A repeat click is dropped in the model, not only by disabling the button, so no path sends a duplicate.

**Why views, not time.** The accepted state ends on a count of views (3), so it is testable without a clock and cannot stick forever if masond's answer changes nothing the sidebar shows.

**Scope.** Commands about one task. Commands with no task (pause, resume, limits, add task) and `raw` commands are not tracked yet.

**Trust.** Client sources only; nothing in `PROTECTED` and not `clients/features.toml`.

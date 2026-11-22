# 0812: the monitors set the placement mode through a typed command, protocol 2

Status: accepted (task #834; protocol and client UI only, no gate).

The placement mode (decision 0809) could only be set with the CLI. Decision: a typed `SetPlacement` command (global, project, task; a mode or `default`) is mapped by `masond` onto the CLI's own endpoints, so the owner-only check and the writers are the same code. Views carry the effective mode and its source (`Placement`), and a task row says when it overrides its project.

- `PROTOCOL` becomes 2 and `MIN_PROTOCOL` stays 1: every new field is additive and the one new command cannot be sent by an old monitor, so old monitors keep working and stay read-only for placement. Raising the minimum would have locked them out entirely.
- `default` for a project removes its `[remote_runs] mode` and the old `fallback`; there is no `default` for MASON.
- A project with no `[remote_runs]` always runs here: its placement is `settable = false` and no control is offered.
- The pending command for global and project levels is kept under a synthetic key (above every task id and batch key), so the control shows "Changing where runs go…" until the next views arrive.

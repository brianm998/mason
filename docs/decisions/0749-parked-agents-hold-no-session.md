# 0749: A parked agent holds no session, but the number in agent steps is capped

An agent waiting on a `run_check` gives its lease back. Admission still reserved a session for it as if it were about to launch, so with remote checks (minutes to an hour) parked agents filled the limit and new tasks never started.

Decision: a task whose agent is parked on a check is not reserved a session. A task between steps or about to launch still is. When the check returns the agent is reserved again, so it is served ahead of new admissions.

Why a cap: without one, every parked agent frees a slot for a new task, which parks in turn, and the number of worktrees and tasks in agent steps grows without bound. At most `llm limit + park_extra` tasks are in agent steps; `park_extra` defaults to the limit (`park_extra` in the project registration file).

Consequence: when many checks return together, more agents want sessions than exist; the extras wait for one, as any step does.

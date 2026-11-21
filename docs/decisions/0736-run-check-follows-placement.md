# 0736: A develop agent's check is placed like any task run

`run_check` built every spec with `place: None`, so `executor_for` always answered Local (#755).

**Chosen.** Suite specs carry the task and suite, so `place_of(…, "check")` decides; the plain project-test command has no suite and follows the global switch.

**Not chosen.** Forcing every check remote: it would ignore a task's choice of here and a suite's `remote = false`.

**Not verified.** Nothing was built or run; the local slot wait before a remote run is untouched.

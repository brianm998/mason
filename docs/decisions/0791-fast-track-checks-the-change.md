# 0791: a fast track still builds the change and runs the tests it adds

Status: proposed (verdict logic in jobs.rs and landing.rs; the owner approved it in chat on 2026-11-21)

Decision: a task skipped with `mason task skip --all` skips the suites, not its own change. `run_check` builds each selected suite and runs only the tests the diff adds, by name (new `#[test]` fns, new or edited exit scripts); the landing build floor does the same for the batch's skipped tasks and blames a red by the files that add the failing tests. `submit_change` refuses a diff that has no source file in it once, when the plan names source files, and accepts the resubmission with a note, which is kept.

Why: #803 and #806 landed with only tests and docs because their code edits did not apply. Under the skip every check said nothing was built or tested, and the floor only proved that the unchanged tree compiled. Their tests, now on develop, fail.

Alternatives not taken: running the whole suite under a skip (defeats the fast track); a check only at landing (the agent gets no warning while it can still fix it).

Consequences: a fast-tracked change that does not build, or adds a failing test, is red in `run_check` and parked at landing. Names are read from the diff as bare fn names and matched as substrings.

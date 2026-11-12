# 0160: Rerun only the failed tests; flaky when no test failed twice

A red test command was rerun whole, and red twice by exit status was "red every time" without looking at which tests failed. 46 of 61 readable such cases failed two disjoint sets of tests.

**The key.** `rerun_filter` in the project overlay, optional. It is explicit rather than guessed from the command text, because a test step cannot know its runner. Forms: `"cargo"` (append `-- --exact <names>`, or ` --exact <names>` when the command already contains ` -- `), or a template containing `{tests}`, replaced by the space-joined, single-quoted names (the same form as the suites' `rerun`). Anything else is refused when the overlay loads. Without it the rerun runs the whole command, and names are still compared.

**Reading names.** From the whole log (not the 1 MiB tail), up to 32 MiB; a larger log is unreadable. A cut log could hide a failing name and a hidden name must not make a flake. Parsers: libtest and pytest summary lines here, XCTest, Swift Testing, pytest and nextest through `mason_testing::parse` (read-only; `crates/testing` is not edited). The same name in several test binaries is one name.

**Judging.** The tests failing in every run so far are kept (the first run's names, then intersected with each later run's).
- Empty: flaky. The step is green, a `test-r<N>.flaky` note is written as before, and each test that failed in some run is recorded in the `flaky` table with the task. The suite column is the step's name.
- Non-empty after the last rerun: red, naming only those tests. Tests that failed once and passed are recorded as flaky.
- First run's names unreadable, or a later red's: today's behaviour ("red every time").
- A red that is not by exit status alone (timeout, signal, script verdict) is not rerun, as before.

**A filter that ran nothing.** `cargo test -- --exact <name>` that matches no test exits 0. A filtered rerun that is green but whose log does not contain every name it was given is not accepted as proof: the next run is the whole command, and on the last run the step is red with that reason. This is stricter than the old whole-command rerun, deliberately.

**Trust.** Any test failing in both runs keeps the step red. The verdict change is the reviewed part: red twice by exit status with disjoint failing tests is now green.

**`mason health`.** It listed no flaky tests; it now lists those of the last 7 days (`suite::test (n time(s))`), so these rows are visible, which quarantine candidates already read.

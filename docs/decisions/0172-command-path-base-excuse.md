# 0172: A red that also fails at the merge base is excused on the command path

On 2026-11-11 `ConfigRoundTripTests.testEveryStoredPropertySurvivesAJsonRoundTrip` was red on star's develop for five hours. It was charged to every star task that ran (7 red runs); the agents each "fixed" the test, the fixes conflicted, and the owner retried six times. The suites path excuses a failure that is "also failing at the merge base"; the command path (a `command`/`test` step, and MASON's own repository) had no such check.

**When.** A test step on the command path, after the last run, when the tests that failed on every run are known (`persistent`, from decision 0160). If any of them does not fail at the base, nothing changes: the step is red as before.

**What runs at the base.** The merge base of the change and the project's base ref, in a throwaway worktree under the task's directory, made and removed under the repository lock. With a `rerun_filter` it runs only the persistent tests (the same filtered command as the rerun); without one, the whole command. A test step with a `build` runs it first. The result is the names that fail there.

**Excused only when the same test fails at the base.** Every persistent test must be in the base's failing set. One test that is not keeps the step red, even if others are excused. A filter that matches nothing at the base (a test new on the branch) exits green with no failures, so nothing is excused.

**Cached.** Once per (base commit, command, test set): the result is stored in the existing `baselines` table, with a hash of the command as the `suite` column (`cmd:<fnv64>`), so a second task with the same base and the same failure does not run the base again. A run that gave no result (worktree, timeout) is not cached and excuses nothing. Base runs are serialised by a process-wide lock so a second task finds the first's result.

**What it does.** The step goes green; `test-r<N>.excused` in the task's directory has one line per test (`<test>: also fails at the merge base <sha>; fix: task #N`), which `mason task show` lists as `excused`, and the log gets the note.

**One fix task.** For each excused test, under a process-wide lock, an open task of the project whose title has the test name and " fails on ", or an open tracker issue naming the test, is looked for. If there is one, the excuse points at it. Otherwise one task, "<test> fails on <base>", is filed: owner trust, urgent, not explored. Other tasks that hit the same failure (even at a later base) are excused and point at that task until it lands (a terminal task no longer counts).

**Trust.** Excusing a red is verdict logic. A failure is excused only when the same test fails at the merge base. `suites.rs` and `crates/testing/**` are not edited: `baseline()` is not shared in code (it needs a suite and its environment), only its table, worktree handling and repository lock are mirrored in `crates/daemon/src/basecheck.rs`. `jobs.rs` is not protected, but the change needs the owner's approval as a verdict change.

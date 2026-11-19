# 0635: flake in the skipped-batch landing test

The recorded failure output of the three flaky runs was not available to this task, so the failing
line is not confirmed. The likely cause is the daemon's idle debt pass (`crates/daemon/src/suites.rs`,
`pay`: fetch, `worktree add --detach`), about 60 s after start, overlapping the test's own
git calls in `W::task` (`crates/daemon/tests/support/lw.rs`), which went through `sh()` and panicked
on any failure, and the single `on_main` read right after `landed`.

Decision: test-only changes. `W::task` runs `fetch` and `worktree remove` through a bounded retry
(`retry_git`, 8 tries), as `add_worktree` already does, and `on_main` is awaited with `wait_for`
before being asserted. No assertion was removed, no timing assertion added, `MIN_WAIT_SECS` and
`crates/daemon/src` are unchanged. If the test flakes again, the next step is to wait for the
`step:test` debt of the first task to close before filing the second.

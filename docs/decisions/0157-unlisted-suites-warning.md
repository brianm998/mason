# 0157: Warn about suites bound to the test step that no step lists

A test step runs suites only when it lists them; otherwise it runs the project file's single `test` command, and bindings, the impact map, same-tree reuse and reruns go unused (star was in this state for six hours on 2026-11-11).

**Decision.** `validate::warnings` warns when some suite's level is bound to `test_step` by a `[binding.*]` and no `type = "test"` step has a non-empty `suites`. It is a warning, not an error, so existing definitions load and run as before. A project with several test steps is fine as soon as one lists suites.

**Why only explicit bindings.** The ticket speaks of levels bound to `test_step`. With no bindings at all, every suite is implicitly a test-step suite; that case is left alone to avoid a new warning on definitions that never meant to bind anything.

**Trust.** `validate.rs` is protected; this adds a warning and refuses nothing.

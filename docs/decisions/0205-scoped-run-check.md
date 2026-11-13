# 0205: run_check may narrow to what the change touches; the test step never does

Develop agents used `run_check` (`cargo test --workspace`, 21-30 minutes in a `test-suite` slot) as a compile-and-test loop: 116 runs over 2026-11-11 and 11-12, about 49 hours of slot time, and two tasks waited 7.7 hours inside one call.

**Scope.** `run_check` takes optional `tests` (name filters), `packages` and `changed`. Names are an allow-list (letters, digits, `_ : . - /`, no leading `-`, at most 20 of 200 characters) because they are substituted into a command; `;`, `$(`, a backtick, a quote, a space or a newline are refused in `contract.rs`.

**What narrows.** On a suite with `rerun`, `tests` runs the rerun template with the names. On a suite with `dev_command` (and optionally `dev_build`), `packages` or `changed: true` runs it with `{packages}` as `-p a -p b`; `changed` computes the list from the worktree's diff and `cargo metadata`, closed over the workspace packages that depend on them. A path no package owns (root `Cargo.toml`, toolchain file, scripts), a docs-only change or no cargo gives no list, and the suite runs whole. A suite with neither key, and a project with no suites, runs whole and the answer says so. `{project_test}` inside `dev_command`/`dev_build` is the project's test command without `--workspace`, so the owner's `CARGO_TARGET_DIR` prefix applies and `-p` is not overridden.

**The test step is unchanged.** It still runs every selected suite's `command`, whole. A scoped green is never recorded: the suites path records nothing, and the single-command path, which cannot narrow, only records a whole-command green (#181's reuse is of that only). The answer says "Scoped (...): this green is not the test step's".

**Prompt.** The develop prompt tells the agent to iterate with `changed: true`, `tests` or `packages`, and to make one unscoped `run_check` at most before submitting.

**MASON's own suite.** `dev_command`/`dev_build` for `cargo-workspace` are written in `.mason/mason.toml` commented out: the daemon that validates the file at landing does not know the keys (as with `build` in decision 0173). A follow-up uncomments them once this change is live. Until then `changed: true` reports that the suite cannot be narrowed that way and runs whole; `tests` already works through the existing `rerun`.

**Why the trust files change.** `contract.rs` (the new arguments and their validation), `suites.rs` (`{project_test}` in the new keys) and `.mason/mason.toml` (the commented keys). No test, limit or gate is weakened: the test step's gate is the same, and only the develop-phase check is narrowed.

**Not verified:** the drop in test-suite wait time (it needs days of real runs); `cargo test -p ... ` on the real workspace through `dev_command` (the keys are not live yet).

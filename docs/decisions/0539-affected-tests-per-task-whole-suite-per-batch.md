# 0539: Tests by change per task, the whole workspace once per landing batch

Status: proposed (waits for the owner: it changes gates in `.mason/**`, `crates/daemon/src/suites.rs` and `.mason/landing.toml`). Supersedes 0205 (the test step runs `command` whole) and 0173 (docs-only changes keep the cargo suite), and extends 0389 (package narrowing).

## Context

Every task's test step and every `run_check` ran the whole `cargo-workspace` suite (156 test binaries, 22-35 min green, over an hour red). On 2026-11-17 there were 194 full runs in 24 h for 10 landed tasks, and 22-25 tasks waited about 1.5 h for each run while no agent ran. Narrowing by package does not help: almost every task touches `mason-daemon`, which holds about 89 of the binaries. The owner's direction (2026-11-17): the full net is too wide per task; run it once per landing batch, over several tasks.

## Decision

1. `crates/daemon/src/affected.rs` selects test targets from a diff, by a static source scan, never by timing. A changed test file selects its own target. A changed `src` file selects its crate's unit tests (`--lib`, `--bins`, `--doc` when the file has doc code) and the integration targets of the crate and of its dependents whose source names the crate and either the changed module or a public item of the file. A target that spawns the crate's binary, includes its source by path, or follows a reference the scan cannot decide is included. A path no package owns, `Cargo.*`, `build.rs`, anything in a test's `support/` directory and any other non-`src`, non-test file of a package select everything, as before. Docs, markdown and `scripts/exit/**` select nothing of their own.
2. A suite's `dev_command` / `dev_build` may use `{tests_for_change}` (groups of `-p P --lib --test T`, one `cargo test` per package, all run, all must pass). `run_check` with `changed: true` runs that set, plus any packages the agent names (`packages`), on the small lane.
3. With `narrow_packages = true` the test step runs the same set plus the smoke list `.mason/smoke-tests.txt`. A narrowed run records no evidence (0389). `swift-clients`, `compose-clients` and `exit-scripts` keep their impact rules.
4. `.mason/landing.toml` gets `[[tier]] name = "batch"; suites = ["cargo-workspace"]` and the ladder `rerun-failed`, `blame-by-impact`, `bisect`. The window is 10 min, so a batch collects tasks that finish together. A red the base shows too is excused by the suite run (also failing at the merge base), so it blames no task.

## What still proves a change before it reaches `develop`

The batch tier: the whole workspace suite over the union of the batch, with a rerun of the failures, blame by impact and a split to find the culprit. A task's own step no longer proves the rest of the workspace; a failure outside a task's affected set and smoke set is found at the batch and sent back to the culprit.

## Consequences and risks

- The scan is a heuristic. A dependent whose test uses a changed crate only through another crate's API is not selected; the batch catches it, one batch later and for the batch's tasks together.
- A red batch costs a whole run plus the split. Splitting still re-runs the whole suite on subsets (see the stage note for what was not changed).
- Decision 0205's reason (a task proves the whole suite) no longer holds; 0173's docs-only rule now means smoke per task and whole per batch.
- #461 (merging daemon test executables) renames targets; the scan and the smoke list work by file and name, so the names in `.mason/smoke-tests.txt` must be updated with it.

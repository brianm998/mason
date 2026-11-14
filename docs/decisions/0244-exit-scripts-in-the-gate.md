# 0244: gate on the exit scripts a change touches, and run all of them nightly

Stage s249. Status: accepted by the owner.

## Context
The test step runs the cargo, Swift and Compose suites. The exit scripts (about 157 that need no real service, person or device) ran only on GitHub CI, whose exit-script shards were red on develop (latest failed run: shards 2 and 3, `scripts/exit/s166.sh`) while the workspace tests were green there.

## Decision
- An `exit-scripts` suite runs the exit scripts the change adds or edits relative to its merge base (`scripts/run-changed-exit-scripts.sh`; the base is `MASON_BASE`, or `origin/develop` where a `run_check` has no such variable). It prints `PASS sNNN` or `FAIL sNNN` per script, which a regex parser reads (`min_tests = 1`); a change with no exit script prints `PASS none`. `[impact]` maps `scripts/exit/**` to `cargo-workspace` and `exit-scripts` (it keeps the cargo suite: a path that was unmapped ran every suite, and this does not relax that). The suite is in the test step's list and, being `level = "unit"`, in the `development` and `test_step` binding.
- The nightly run of every script is the existing `base-health` schedule kind, which runs a command on a clean checkout of the base branch and files only failures that were not in the previous run. What was missing was a guard against filing a failure whose task is still open: `base-health` now skips a failure whose title matches an open task's. The schedule itself lives in the owner's overlay and is not edited here: the lines to add are in the stage note.

## The exit scripts at origin/develop (30d2a98e)
`bash scripts/run-exit-scripts.sh` was run on a clean checkout of origin/develop. It is slow on this machine (the first scripts alone took about an hour with their builds), so the run had not finished when this was written (it continues in the background); the nightly run is the standing way to see all of them. What was seen:

- `s05.sh`: fails here because `crates/adapters/tests/sandbox.rs` `sandbox_exec_really_confines_writes_when_it_is_installed` fails at its start (the same test fails run alone, after 0.1 s; it passed in a sequential workspace run on 2026-11-13). `sandbox-exec` cannot be nested inside this session's own sandbox, so this is the environment, not develop.
- `s03.sh`: failed in a first attempt only because that run used a target directory of its own and the script expects `target/debug/examples/crash_tool`; not a defect.
- `s166.sh` (the script CI also fails): `masond` exits at once with an empty log (`Killed: 9`) in this environment, before any check of the script runs, so the cause CI sees was not reproduced or found. It is listed, not fixed.

## Not done
The full 157-script baseline was not completed. The regression behind the red CI shards was not found. The nightly schedule is not enabled (overlay).

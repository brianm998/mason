# 0243: a runner for the cargo suite's test executables, off by default

Stage s248. Status: accepted by the owner (the suite stays sequential).

## Context
`cargo test --workspace` runs the workspace's test executables one after another; libtest runs the tests of one executable in parallel, never two executables at once. A sequential run on 2026-11-13 took 3204 s (53 min 24 s): 1473 tests passed and 1 failed (`the_retest_builds_holding_compile_then_tests_without_it`, in `-p mason-daemon --test landing`, not examined further). The summed `finished in` time of its executables was 2922 s and the slowest executable took 518 s.

## What landed
- `scripts/test-parallel.py`: lists the test executables (`--no-run --message-format=json`), runs them N at a time (default 4, `MASON_TEST_JOBS`) in their package directories, runs the targets named in `.mason/serial-tests.txt` alone afterwards, then the doc tests, printing each executable's output as one block under cargo's own `Running` line and a one-line summary. Exit status is non-zero if any executable or the doc tests failed. `scripts/exit/s248.sh` checks it on a fixture workspace.
- `[resource.test-small] count = 2` and `small_lane` in `jobs.rs`: a scoped `run_check` (changed, tests or packages) takes `test-small` instead of `test-suite` when the definition declares it, so it does not queue behind a full run. Full checks and the test step keep `test-suite`.
- The suite commands are **unchanged**: the runner is not used by the suite.

## Spike
One 4-way run of all 128 executables (97 integration targets plus unit-test executables) on a clean checkout of the runner's commit, own target directory. Only one run was done, not five: every cargo command waited for a compile slot and the machine was saturated by MASON's own pipeline. The tree was clean on purpose: a first attempt on the moving working tree was thrown away.

| | sequential | 4 at a time |
|---|---|---|
| wall clock | 3204 s | 3191 s |
| summed per-executable test time | 2922 s | 11272 s |
| slowest executable | 518 s | 1066 s |
| tests passed / failed (lines `... ok` / `... FAILED`) | 1473 / 1 | 1462 / 12 |

Eight executables failed in the 4-way run. Each was run again alone, on the same build:

- Pass alone (collisions with load, so in `.mason/serial-tests.txt`): `scheduling` (22 tests), `holds` (11), `wait_classes` (1), `stallwatch` (3), `taskcache_own` (4).
- Fail alone too (not collisions; red or flaky on their own): `landing`: `a_red_reverification_after_the_base_moved_ends_and_does_not_hang`; `notify`: `the_disk_alert_survives_a_store_that_refuses_writes_and_is_delivered_once`; `adapters` `sandbox`: `sandbox_exec_really_confines_writes_when_it_is_installed` (fails after 0.1 s, so most likely the environment this session runs in; it passed in the sequential run). `landing` also failed a different test in the 4-way run than alone.

The count of passes is lower in the 4-way run because the failing executables stopped counting their other tests as passed in the same way; the 1473 baseline had one failure, the 4-way run had twelve.

## Decision
The suite stays sequential. MASON already runs up to four suites at once and the CPU is the bottleneck, so the runner does not shorten a suite under load: the wall clock was the same, and each executable took about four times as long, which caused timeouts. The runner stays in the repository. To switch it on, point the cargo-workspace suite's `command` (and `dev_command`) at `python3 -I scripts/test-parallel.py -- {project_test}` (with `{packages}` for `dev_command`) and set `MASON_TEST_JOBS`, if the machine has headroom.

## What it does not cover
A second run was not done, so other collisions may turn up; they would be handled by extending `.mason/serial-tests.txt`. A collision that is missed shows up as a flaky red, because `rerun` stays sequential plain cargo. The doc-test run is cargo's own and is not parallel.

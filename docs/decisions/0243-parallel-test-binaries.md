# 0243: run the cargo suite's test executables in parallel

Stage s248. Status: accepted by the owner.

## Context
`cargo test --workspace` runs the workspace's 97 test executables one after another. A measured sequential run on 2026-11-13 (machine loaded by the pipeline) took 53 min 24 s: 1473 tests passed and one failed (`the_retest_builds_holding_compile_then_tests_without_it`, in `-p mason-daemon --test landing`).

## Decision
- A runner (`scripts/test-parallel.py`) runs the executables four at a time (`MASON_TEST_JOBS` or `--jobs`), the targets in `.mason/serial-tests.txt` alone afterwards, and the doc tests last. The suite's `command` and `dev_command` use it; `rerun` stays plain cargo.
- A scoped `run_check` takes `test-small` (count 2) instead of `test-suite` when the definition declares it, so it does not queue behind a 20-minute full run. The test step and full checks keep `test-suite`.

## Spike
Per the coordinator, two 4-way runs were done instead of five, because every cargo command waited minutes for a compile slot. Results are in the section below.

## Spike results
(To be filled in from the runs.)

## What it does not do
It does not shard within an executable (libtest already does that), and the one `test-suite` slot now holds four processes: the machine's load is the limit, not the slot count.

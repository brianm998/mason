# 0700: reuse a green cargo test target when its inputs are unchanged

Status: awaiting the owner's approval (it touches `crates/daemon/src/suites.rs`, which is in `PROTECTED`: skipping a test on a recorded green is verdict logic).

## Decision

After a suite's build, each cargo test target gets an input key. A target whose key is recorded green is left out of the run and named in the step note ("reused ... green on #N (tree T) with the same inputs"). New or changed tests have a new key, so they always run.

## Why the key is safe

A wrong key may only cause a rerun, never a skip, so it is over-inclusive:

- The dep-info of an executable lists only its own crate's files, so the key also takes the dep-info of the library of every workspace package in its dependency closure (and the manifest and `build.rs` of each).
- What dep-info cannot see: every file under the package's `tests/`, every file outside `crates/`, `clients/`, `docs/` and markdown (so `Cargo.lock`, the toolchain file, scripts, `.mason/`), and the inputs of each workspace binary that the package's `tests/` or `src/` name (`CARGO_BIN_EXE_*` or the binary's name as a word).
- Paths are workspace-relative, so two worktrees with the same content agree.
- A target with no dep-info, a group with a test filter, and doc tests have no key and run.
- A key recorded red stays red even if a later run on it is green (flaky). A suite with known-red tests reuses and records nothing.
- Dep-info files are matched by name and by a source under the package's directory (two packages may each have a test `it`); stale files for the same target are included, which only adds inputs.

What it can still miss: a test that reads something neither named in the `tests/`/`src/` text scan nor tracked outside `crates/` (a file in another crate's directory, the network, the clock, machine state). The periodic full run is the check.

## The periodic full run

With `reuse_green = true` a landing batch reuses targets. The first batch after a daemon start, and then once every `reuse_full_hours` (default 24), runs every target with reuse off, still records, and on a red drops the suite's recorded keys and says in the batch note that a reuse which hid the red is a bug against `reuse.rs`. Off by default. Task test steps reuse without a setting, since they run a narrowed selection and the batch still runs the whole suite.

## Rejected

Reusing by tree hash alone (the existing `proven`): any change anywhere reruns everything. Reuse per `it` module needs the coverage-map ticket.

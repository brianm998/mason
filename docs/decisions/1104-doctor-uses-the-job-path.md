<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1104: the node doctor checks tools with the job's PATH

## Context

On 2026-11-25 a node set up from docs/nodes.md failed its doctor with `cargo does not run`, although rustup had installed it in `~/.cargo/bin`, where jobs find it.

## Decision

One function, `node_run::job_path`, builds the `PATH` for jobs and for the doctor. The doctor runs each tool from the node user's home. `cargo` stays a blocking check; only where it is looked for changed. The documentation also sets a `PATH` in the plist and in the `mason` wrapper, but the doctor depends on neither.

## Not chosen

- Only documenting a `PATH`: the daemon's check would still differ from what a job gets.
- Resolving `~/.cargo/bin/cargo` directly: a second rule to keep in step with the job environment.

## Test worlds (added after the GitHub run timed out)

The owner approved changing the test harness in this task: a test holding a world no longer waits 600 s for its next one. The wait ended by going ahead without a slot, so the limit's maximum overshoot is the same, only reached sooner. Raising the 60-minute limit was not done.


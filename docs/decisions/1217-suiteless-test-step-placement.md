<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1217: a test step without suites follows placement

Task #1237. A project with a `test` command and no suites, in `remote` mode, held a node's unit for
its test step and then ran the command on the coordinator.

## Decision

The step is placed as a check of its task is: `executor_for` for the task with the step named as a
suite, so the task's own `run_on` wins, then the project's mode, then the global switch. Off the
machine, it runs through `hostedcheck` (one run with its build), which already waits for a node in
`remote` mode, treats an unreachable node or GitHub as no result, and attaches to a run out after a
restart. Everything else about the verdict stays the local loop's.

`command` and `gate` steps are not tests and stay on the coordinator. Baseline runs are unchanged.

## Alternatives not taken

- A new `CheckSpec` field for "plain, for this task": it would touch every literal, including
  `landing.rs` (protected). The step-as-a-suite carries the task through the existing place.
- Running the step's own loop on the node under `node_exec` directly: duplicates the waits and the
  outage rules `hostedcheck` has.

No protected file changes.

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1204: file edits are kept as a JSONL timeline in the log directory

The agent's `Edit`, `MultiEdit` and `Write` calls are recorded per step run in
`edits-<run>.jsonl` beside the task's other logs, not in the store: the texts are large and are
read only on request. Each text is capped at 8 KB and each run's file at 4 MB. `task show --edits`
prints them as diffs; reviewers and diagnosis get only per-file counts and a "reverted" flag,
fenced as data, never the edit text (an agent wrote it). The monitor tab is left for a later
task.

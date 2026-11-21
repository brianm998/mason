# 0737: the run snapshot keeps the base branch's workflows, never the task's

Status: accepted (task #756)

Tests such as `ci_gates` read `.github/workflows/ci.yml`, which the snapshot dropped. The snapshot now restores the **base branch's** copy of every workflow file (read from `base_ref`). The base-only rule is the trust boundary: a task cannot add or change a workflow that runs on `mason/run/**` with the repository's secrets. The run refuses if a base workflow other than ours fires on `mason/run/**` (the same check `mason doctor` makes).

Consequence: a task that edits `ci.yml` together with a test that reads it is tested against the base's `ci.yml` on GitHub.

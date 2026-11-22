# 0800: the coverage map is made on GitHub

Status: accepted (#780).

With remote on, the coverage job runs as a `covmap` run on GitHub and the daemon downloads `covmap.kv`; it leases `gha`, not `test-suite`. The job's own code is reused as `masond covmap-job`.

`llvm-tools` is installed by the run's command (`rustup component add llvm-tools &&`), not by a step in `.github/workflows/mason-run.yml`. Reason: the workflow is a protected file, and a command in the pinned run needs no workflow change. Rejected: a workflow step keyed on the run's kind, which waits for the owner's approval. If the owner prefers it in the workflow, only `REMOTE_COMMAND` changes.

With remote off the local job is unchanged. `mason doctor` now notes a missing `llvm-tools`.

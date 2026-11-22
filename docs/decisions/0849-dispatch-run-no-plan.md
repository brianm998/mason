# 0849: a GitHub run starts by dispatch, with its shard list and runner as inputs

Status: accepted (owner asked for it on 2026-11-21; touches `.github/workflows/**`, protected: it changes how a run is started, not what it runs or how it is judged).

A run was two waves of jobs: a `plan` job read `.mason/run.json` and output the matrix and runner, then `run` took them. Each wave waited for a runner, and `run` joined GitHub's queue only when `plan` ended.

Decision: `execute` pushes the snapshot ref, then dispatches the workflow on it (`POST /actions/workflows/<file>/dispatches`, `workflow_dispatch`) with the inputs `runs_on`, `matrix`, `timeout_min`, `sharded` and `toolchains`, all taken from the same JSON as `.mason/run.json`. The `plan` job is gone: `run` has `runs-on` and `matrix` from the inputs, so its jobs (`run`, or `run (1)` to `run (k)`) exist from the first look. The run is still found by head SHA and branch.

Trust: the workflow in the snapshot is still the base branch's copy. Its first step compares every input with `.mason/run.json` (pinned by MASON, from the project's definition) and fails the job on a difference, so nothing after it runs. The runner label itself cannot be checked before a runner takes the job; only the token's holder can dispatch.

Never twice: the workflow no longer runs on push. `snapshot` refuses a base-branch workflow of ours that does, and `mason doctor` fails on it. The record keeps `dispatched`; a restarted daemon that finds a record with no run looks for a run of the commit first and dispatches (once) only if GitHub has none.

Doctor: checks that the workflow accepts `workflow_dispatch` and has no push trigger on run refs, and that GitHub lists it as an active workflow. Actions write access cannot be probed without dispatching; a refused dispatch ends the run as an environmental failure (the ref is deleted).

# 0699: a remote executor on GitHub Actions, beneath the existing run seams

Status: accepted (pending owner approval: `crates/daemon/src/suites.rs` is protected)

The Mac runs hot (#693) and more concurrency only added heat. Runs go to GitHub-hosted runners
instead, which cost nothing for public repositories. This decision builds the executor only;
#698 chooses where each run goes.

**Where the seam is.** Under the callers, not in them. `run_hosted_resuming` (suites) and
`hostedcheck::collect` (checks) ask `remote::executor_for`, which answers `Local` for every
run, and otherwise return the same `Ran` / `CheckOutcome` as before. Everything that judges a
run (parsers, verdicts, evidence, reruns, flaky, baseline, known-red, debts, progress) sits
above the seam and is unchanged. A lost remote run is run again: resuming (#621), narrowed
resumes and the pty wrap stay local features.

**Trust.** GitHub runs the workflow found in the pushed commit, so a task could make it report
green. The snapshot therefore replaces `.github/workflows/mason-run.yml` with the base
branch's blob, removes every other file under `.github/workflows/`, and writes
`.mason/run.json` from the pinned definition (suite, command, `rerun`, `results_file`, limit,
runner and MASON's own `MASON_*` environment). A runner learns nothing about how it is judged from the task's tree, the
same trust as a local run, whose command also comes from the definition. Only `MASON_*`
variables are sent, because the commit is public.

**The snapshot is a commit made with a temporary index** (`read-tree`, `add -A`, overlay,
`write-tree`, `commit-tree -p HEAD`), so a dirty worktree is run as it is, untracked files
included, and its index and `HEAD` are never touched. Ignored files are not included: a
checkout on the runner would not have them either. The parent keeps the push small.

**Output.** A job log keeps only the last 256 KB, too little for a suite's per-test lines, so
the workflow uploads one artifact (`output.log`, `result.json`, `results/**`) and the daemon
downloads it. The zip is read with the system `unzip`, not a crate: `deny.toml` stays as it
is, nothing is added to the dependency tree, and macOS and the Linux runners have it. Only
three named members are extracted, and only regular files are read, since the zip is the
runner's output and so untrusted. `Http::download` writes a body to a file byte for byte
(`Curl` uses `-o`); the redirect GitHub answers with is followed without the token.

**Limits and outages.** A limit counts from when a runner took the job (the first poll that saw
it), not from the push; `pickup_timeout` counts from the push. No runner in time, an outage as
long as that, a refused token, `startup_failure`, a run that ends without an artifact and a
base branch without the workflow are environmental: a wait charged to no task. With
`fallback = "local"` the run takes the local path. The ref is deleted whatever the end was, and
the run is cancelled (only on `mason/run/**` or `mason/ci/**`, `may_cancel`) when the owner
skips or forces.

**Restarts.** `<id>.remote` beside the log holds ref, SHA, run id and whether it was
collected. A daemon that restarts finds the run by that id and collects it; it never starts a
second one for the same id.

**Not decided here.** Which runs go remote (#698), per-task choice (#699), runner pools (#700),
shards (#701), landing (#702), setup (#703), agents' own commands (#704). `MASON_TEST_FORCE_REMOTE=1`
in the daemon's environment is the only way to reach the remote path until then.

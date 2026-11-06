# 0006: Readings made while building the testing system (Stage 9)

Where the plan or design left room, Stage 9 chose the following. Any can be overruled.

- **`mason-testing` is pure and the definition crate depends on it.** It holds the
  model (suites, environments, bindings, the impact map, signatures), the parsers, the
  verdict and classification rules and the gates. It is handed text and returns
  answers; running commands, leasing resources and storing evidence stay in the
  daemon. The definition's `[suite.*]`, `[environment.*]`, `[binding.*]`, `[impact]`
  and `[signature.*]` tables build its types.
- **A test step runs suites if it names them (`suites = [...]`) and its command
  otherwise.** The older exit-code-only test step still works, so projects migrate
  one step at a time. A step names suites or a command, never both.
- **What "green requires proof" means.** A suite is green only if its parser found
  at least `max(min_tests, 1)` executed tests (skipped ones do not count), none
  failed, and the command's exit status is 0. Fewer executed than that, with a clean
  exit, is *vacuous*, and vacuous is red. A non-zero exit with no failing test named
  is red too (the tool said something the parser did not understand). A suite may
  read a results file instead of its output (`results_file`); a file that was not
  written is red.
- **Skipped means not executed.** A skipped or ignored test proves nothing, so a run
  of nothing but skips is vacuous.
- **Red is classified in a fixed order, so the task is blamed last.** Environmental
  (any signature matches the output, even when no test is named), then quarantined,
  then flaky (the failing tests are run again straight away, only those, using the
  suite's `rerun` template or the whole command), then baseline (the persistent
  failures are looked up at the *merge base*), and only what remains is the task's.
  A mix is split: the task is told what is its own and what was set aside.
- **Flaky and baseline and quarantine all pass the step, and downgrade the closing
  verdict to `unverified`.** Flaky failures go in a log that proposes quarantine
  after three in a week. A baseline is computed once per merge-base commit and suite
  (in a throwaway worktree) and cached in the store with every test name there, which
  the negative control reuses.
- **Environmental is a wait, not a failure.** The step records a wait ("environment
  problem, not the task's fault: ...; retrying") and tries again every
  `--env-retry-ms` (default 30 s) until it works, the task is cancelled or the suite
  is skipped. It holds the resources it already has meanwhile. There is no alert
  system yet (that arrives with the notification stages), so the wait text and the
  daemon log are the alert.
- **Closing verdicts rank `unproven < unverified < untested < verified`; a task's is
  the worst of its suites.** `verified`: every selected suite is green with proof or
  has green evidence for the exact tree. `unverified`: something was set aside (a
  skip, an excused failure, a flaky pass). `untested`: no suite was selected for the
  change (the impact map mapped it to nothing). `unproven`: a suite ran and proved
  nothing, or could not run, or is red. The closing note names each suite's outcome
  and, for anything not simply green, why. The verdict is recorded per task and never
  blocks a green step by itself.
- **Evidence is keyed by the tree hash of the change's HEAD and the suite name, and
  only green is remembered.** Identical code therefore never tests twice, and
  restarting loses nothing. A red is never reused, since it may have been flaky.
- **Impact map and bindings.** The impact map is checked against the files the task
  branch changes against its base. A path no glob names selects everything. A suite
  whose level is not bound to `test_step` is not run by a test step; with no
  `[binding.*]` tables every suite runs in the test step and nowhere else. The other
  bindings (`development`, `batch`, `after_landing`, `schedule`) are parsed,
  validated and queryable; the consumers arrive with the stages that have those
  moments (landing batches, scheduled runs), and `run_check` is not wired to
  `development` yet.
- **A step holds what its suites hold.** The step's lease is the union of its own
  `uses` and those of each selected suite and its environment, so a suite needing
  `compile` makes the step wait for it before any timeout starts.
- **Force skip is a debt, and a debt is a row.** `mason task skip` records a debt
  (`suite:NAME` or `step:NAME`, with a reason, an owner and whether to run it after
  landing) and the running suite notices within a tenth of a second and is stopped;
  green lanes keep their results. A skip stays in force for the task's later rounds.
  `mason debt run N` runs the suite on the base branch's tip in a throwaway worktree;
  green pays the debt, red leaves it open and says why. A debt marked `--deferred`
  runs after the task lands.
- **The gates are step options (`gates = [...]`), not a separate step type.** `gate`
  steps still cannot run (they validate as language, refused when applied). The
  assertion scan reads the diff from the merge base: in test files (the suite's
  `test_paths`, or any path containing `test` or `spec`) it flags a net loss of
  assertions and any added disabling marker, and passes if a commit message in the
  change mentions "assertion" and gives a reason (six words or more). The acceptance
  gate wants a table, `.mason/acceptance.json` in the worktree, `{criterion: [test
  names]}`, covering each criterion in the plan, with each named test run and
  passed. The negative control runs the suite at the base with the change's test
  files laid over it and requires every test that is new (in the change's results
  but not in the base's) to fail there.
- **Environments.** `local` (setup, health, teardown) and `compose` (Docker Compose
  up with `--wait`, always `down -v`). Health is retried until its timeout. The four
  phases (lease, setup, health check, teardown) are the interface simulators,
  emulators and devices will implement in Stage 25. Compose is tested with its
  commands, not with Docker itself.
- **Quarantine needs an owner and an expiry no more than 90 days away**, in the CLI,
  the API and the library; an expired quarantine excuses nothing.
- **Playwright and Cucumber results are JSON; Playwright's `flaky` counts as a pass,
  a Cucumber scenario with an `undefined` or `pending` step is a failure.** The
  others are read from their usual console output. Anything with no reader can use
  the `regex` parser, whose `pass_regex` and `fail_regex` each count one test per
  matching line, named by the first capture group.

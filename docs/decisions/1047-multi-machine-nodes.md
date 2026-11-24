# 1047: Running work on other Macs ("nodes"): readings and choices

The plan is `docs/multi-machine-nodes-plan.md`; the owner approved it on 2026-11-23. This record
holds what building it settled where the plan left room, and what turned out different. Any item
can be overruled.

## Phase 0

- **Two questions in `decide`.** `may_leave` holds the rules every off-machine target shares (no
  `[remote_runs]`, `remote = false`, a leased device, `exclusive`, a failed setup check);
  `off_machine` chooses where. Only GitHub is a target so far. The old decision table passes
  unchanged, and `a_run_that_may_not_leave_stays_and_one_that_may_goes_off_machine` pins the split.
- **"Is it GitHub" became "is it off this machine" in three places** where the question was never
  about GitHub itself: the build-as-first-part-of-the-command rule (`suites.rs`), `suite_goes_remote`
  (a landing batch's hold) and the bisect's side-by-side check (`landing.rs`). The sites about
  GitHub's own pool (`gha_full`, the coverage map, `pool_needs`) stay GitHub-only.

## Registering one project on two coordinators is unsupported

A project has exactly one coordinator and so one landing queue; no landing lock crosses machines
(plan §3.13). `mason project add` warns when a paired node reports the same remote URL.

## Phase 2

- **The environment audit.** A job's environment is built on the node, from nothing. *The
  coordinator sends* only `MASON_*` names made of letters, digits and `_` (`run_json` filters them
  and the node filters again): `MASON_PROJECT`, `MASON_STEP`, `MASON_ATTEMPT`, `MASON_BRANCH`,
  `MASON_BASE`, `MASON_TITLE`, `MASON_HELD`, `MASON_TASK` (the task number), and whatever a run
  sets for itself such as `MASON_SKIP_SWIFT`. *Dropped*, because each is a path on the
  coordinator's disk: `MASON_TASK_CACHE`, `MASON_WORKTREE`, `MASON_RESULT_FILE`,
  `MASON_PROGRESS_FILE`. *The node sets its own*: `PATH` (shims first, then `~/.cargo/bin`, the
  daemon's `PATH`, the system's), `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`, `LANG`,
  `CARGO_HOME`, `RUSTUP_HOME`, `DEVELOPER_DIR` and `JAVA_HOME` when its daemon has them,
  `MASON_WORKTREE` (the slot), `MASON_NODE=1`, `MASON_TASK=node` when none was sent,
  `CARGO_TERM_COLOR=never`, `NO_COLOR=1`, `GIT_TERMINAL_PROMPT=0`. Nothing else of the node's own
  environment reaches a job. The build output goes where the project's scripts put it
  (`$HOME/.cache/mason-target/<checkout directory name>`), which is why a slot's name is
  `<project>-slot-<k>`: unique across projects, and the same every time, so builds are warm.
- **Toolchains.** A node's toolchains are read from `rustup toolchain list`, the
  `/Applications/Xcode*.app` names and `/usr/libexec/java_home -V`, at most every ten minutes. The
  coordinator sends a job only to a node that has what the suite is known by (`remote::toolchains`:
  Rust for most, Xcode for `swift*`, Java for `compose*`) and the Xcode version a command names. It
  does not read the Rust version a project pins: a node runs the project's `rust-toolchain.toml`
  through rustup, as a local run does, and rustup fetches a missing toolchain.
- **The artifact is JSON, not a tar.** `GET /node/jobs/{id}/artifact` is `{result, results:
  [{path, b64}]}`. The coordinator writes a results file only when its path is plain and relative
  (no `..`, nothing absolute), so no archive of a node's is ever extracted on the coordinator.
- **Shims on a node.** `caffeinate` loses `-u` (it wakes the display) and `xcodebuild` refuses a run
  that selects a bundle with `UITests` in its name by `-only-testing:`. A scheme that includes a
  UI test bundle without `-only-testing:` or `-skip-testing:` cannot be told from its arguments:
  suites for such projects keep `remote = false`.
- **A node does not use its own broker for jobs.** A job counts against `max_jobs` and the other
  rules of availability; it holds no slot of the node's own `compile` or `test-suite`, which belong
  to the node's own pipeline (a node need not have one).
- **Capacity on the coordinator** is `Broker::set_pool_units("node:<nid>", n)`, the call a device
  pool's reading uses, since `sched.rs` is protected and `set_limit` writes the owner's limits file.
  `n` is what the node has free plus what this coordinator holds there, and at least one, so a step
  queued for a node that went away is let through to find that out and fall back.
- **A fall-back from a node goes to this Mac.** When no node has room the run is environmental and
  takes the local slots a run placed here takes (`fallback_lease`), as a GitHub run that found no
  runner does. It does not try GitHub next: placement already chose a node because it had room, and
  `prefer = "github"` puts GitHub first.

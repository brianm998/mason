# 0170: A `ci` schedule kind dispatches a slow workflow, follows it and files its reds and flakes

Star's full Gherkin matrix takes 3 to 4 hours, so it runs once a night, dispatched by starpipe, which then reruns the failed jobs and files tickets. The owner wants no logic in two places, so MASON takes this over as a schedule kind. It is **off unless a project's overlay has `[schedule.<name>] kind = "ci"`**; without one, no call is made to the CI host (the kind is only ever reached through a configured schedule).

**Where it is configured: the overlay, like the other kinds** (`~/.config/mason/projects/<id>.toml`), because the dispatch is the owner's machine policy and the cutover from starpipe is a machine-config change. The project's `[ci]` must be `github-actions`; otherwise a run says so and does nothing.

| key | default | meaning |
|---|---|---|
| `kind` | | `"ci"` |
| `workflow` | required | the workflow file, `gherkin.yml` |
| `ref` | the base branch | what to dispatch on |
| `inputs` | none | a table of strings sent with the dispatch (`{ features = "" }`) |
| `run_name` | none | the run-name the dispatch gives its run; how this schedule's runs are told from others (without one, any `workflow_dispatch` run counts) |
| `at` | | `"HH:MM"`, once a day in local time |
| `every` | | instead of `at` (not both; one of them is required) |
| `paths` / `paths_from` | none | inline globs, or a workflow file whose `on.pull_request.paths` is read at the tip (not both). With neither, any changed file is relevant. A `paths_from` file with no such list is an error and dispatches nothing: it is never read as "everything" |
| `group_by` | each job its own group | `"<group>/<leg>"` (star's `feature/leg`) or `"<leg> · <group>"` (starpipe's old names); split at the first separator |
| `max_tickets` | 6 | new tasks per run, reds first |

An unknown key, a ci key on another kind, `at` with `every`, or `paths` with `paths_from` is refused when the project loads.

**Deciding (starpipe's `decide`).** Over the workflow's runs on the base branch that carry the run-name: one in flight, skip; the newest finished run (success or failure, not cancelled) tested exactly the tip, skip; no green run ever, dispatch; otherwise dispatch only if a file changed between the last green run's commit and the tip matches the globs (GitHub semantics: `**` crosses `/`, `*` does not); a green commit not in the repository dispatches. `force` is not carried over.

**Following.** A dispatch answers with no run id, so the follow state starts as "locate": the runs that existed before are remembered and a later tick takes the new `workflow_dispatch` run. This avoids sleeping in the job. The state (`schedules.follow`, migration 26) is a small JSON text, so a restart loses nothing. A tick looks at a followed run every schedule tick (30 s) and does not count as the schedule running. Phases: `locate`, `await1` (first attempt), `await2` (after the rerun). When the first attempt finishes, the failed jobs and their log tails are **captured first** (the rerun replaces them), then `rerun-failed-jobs` is called once. A run that was rerun by hand is not classified. A followed run that has not moved after 3 days (injected clock in tests) is dropped. A refused rerun is tried again on the next tick.

**Judging.** Per job that failed on attempt 1: unfinished on the rerun, cancelled (either attempt), or judged infrastructure by `ci_infra::judge` (0148) on the rerun's failure, or on attempt 1 followed by a green rerun: **infrastructure**, never filed. Red again: **red**. Green on the rerun: **flaky**.

**Filing.** One task per group and kind (legs grouped; a group with a red leg and a flaky leg files two). Reds are `Normal`, flakes `Low`, trust `Owner`, filed through the same `emit_task` the base-branch watch uses. The title is the full `workflow/job` keys, `CI/alpha/linux red twice on main` (or `... flaky on main`); a long list falls back to `workflow/group ... (N legs)`. Those are the keys the watch's dedupe looks for, and the dedupe is the watch's own `named_keys`: a group is not filed if an open task's title or an open tracker issue's title or body contains any of its job keys or its group key. Because the watch's titles contain the same keys, whichever files first stops the other. A covered group does not use the cap. Over the cap, reds are filed before flakes, so a flake is what is left unfiled.

**Choices the ticket left open, and what they cost.**
- A new `WorkflowRun` type in the adapter instead of extending `ActionsRun`: the watch's code and tests build `ActionsRun` literally and are untouched.
- A failure to reach GitHub or git at the start of a night **consumes the night** (`last_run` is set before the run, as for the other kinds); the status says why and `mason schedule run <name>` retries. Starpipe retried after 10 minutes. Retrying every tick would hit a missing token or a down host every 30 seconds.
- A failure to file marks the group "could not file" and the follow ends; the group is not retried (the dedupe would stop a duplicate, but a store that cannot take events will not recover by being asked again). Starpipe's pipeline-wide open-ticket cap (`max_open_tickets`) is not carried over; `max_tickets` is the only cap.
- No notification on a finished run: the result is the schedule's `last_status`, the tasks, and `mason status` while in flight. Starpipe also sent an alert.
- The local offset for `at` is read from `date +%z` each time it is needed, so a change of offset (daylight saving) is followed without a restart.

**Cutover** (the owner's, written out in `docs/stages/s175.md`): add the schedule to star's `star.toml`, set `gherkin_nightly_enabled = false` in starpipe's config, restart both daemons. MASON changes nothing in star.

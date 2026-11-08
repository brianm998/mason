# MASON working on MASON

MASON can land its own changes. This page says what that means, what stops for you, and how to turn it on. **It is off until you register the repository as a project**: nothing in the repository does that.

## Turning it on

Copy `docs/examples/mason-self.project.toml` to `<config dir>/projects/mason.toml`, point `path` at your checkout, and start (or restart) `masond`. The `[self]` section is what makes the project MASON's own. The repository's `.mason/mason.toml` is its pipeline and `.mason/landing.toml` how it lands (a batch at a time, straight to `develop`, after `cargo test --workspace`).

## What stops for you

Before a self-change may join the landing queue, the **running** daemon looks at its diff. It stops, and waits for you, if the change

- touches a **gate** (`.mason/`, `scripts/exit/`, `scripts/check-*.sh`, `scripts/gen-*.sh` (run by the daemon when changes land), CI workflows, `deny.toml`, `clients/features.toml`, the golden logs), **verdict logic** (`crates/testing/`, how suites run, the self-test), a **guard** (the network guard, the step contract, webhook verification, how work is merged, batched and promoted, updates and rollback), a **budget** (resources, the scheduler, leases), **trust** (outside-text fencing, who may answer, trust sources, configuration layers, definition validation), or the **self-change rules** themselves (`protect.rs`, `selfguard.rs`); or
- **weakens a check anywhere**, even in files nobody listed: a removed assertion or test, a deleted test file, an ignored test, a raised limit (a number that went up under a name like `max`, `limit`, `timeout`, `budget`, `retries`), a lowered floor (`min_*`), or a broadened skip (`|| true`, `exit 0` in a script, `continue-on-error: true`, `skip = true`).

The scan reads only what the task adds on top of the freshest base (a base merged into the task does not count). A *new* file under `scripts/exit/` is not held by its path, and `|| true` after `kill`, `rm`, `wait`, `trap` or a probe is not a broadened skip; the hold text is one short line with file and line of each finding (decision 0019).

Everything else lands freely.

The list is compiled into the daemon. A change to the rules cannot take effect without passing the rules that are running, and the files that hold them are on the list. A change that adds a file claiming nothing is protected is held anyway, because the running rules do not read the candidate. You can **add** globs under `[self] protect = [...]` in your project file; no change can remove one.

**By default nothing waits for a person (`[self] holds = "review"`).** Whether a change is valid is decided at test and review, where the context is, not by someone reconstructing it at the end. A change that weakens a check (a removed assertion, a raised limit, a skipped test, an `|| true` on a gate) goes back to development with the file, the line and what was found, like any red test, and fails after the step's rounds if it is not fixed. A change that only touches a protected file lands once its tests and review are green; `mason self` lists it as approved by `review`, with what it touched. Set `holds = "owner"` in `[self]` to get the Stage 17 behavior described below, where either waits for you.

The task shows why it waits (`mason task show N`), the alert says so, and `mason self` lists what is held. Approve with `mason task approve N`, from your own identity: anyone else is refused and the attempt is recorded. **An approval is for that exact diff**: a different diff is held again.

## Limits

Open self-tasks, tasks filed per week, and agent steps per period (the cap on spend: token cost is not recorded yet, so steps are what is counted) each have a limit in `[self]`. Filing past a limit is refused with the number and the limit, by the command line, the API and the tracker import (an issue left unimported is looked at again later). `mason self` shows where each stands. The urgent fix task the landing freeze files is not counted: restoring the base comes first.

**The weekly cap counts only work MASON starts for itself.** `per_week` counts tasks whose trust is not the owner's: agent proposals and outside issues you labeled. Tasks you file (the command line or API as the owner, the monitor, a tracker issue by an owner name) are not counted, and neither is a task cancelled before any agent step started. `mason self` reads "N of 10 this week (M of yours not counted)", and the refusal says how many of the counted are agent or outside tasks. **Limits are for resources and for work MASON starts for itself, not for how many tasks you file.** `max_open` and `max_steps` count and refuse only tasks that are not yours, like `per_week` (decision 0026). `mason self` shows how many of yours are open ("N of yours open, not limited"). The cooldown after a rollback still stops every filing, yours included, and the holds on protected files stay. **An owner identity can therefore file any number of tasks.** What bounds your own work is the machine: at most `llm:claude` agent sessions run at once (default 10; set `[llm] claude` in `resources.toml` or `mason resources limit llm:claude N`, which survives a restart), and a local compile starts only while CPU use is below `compile_cpu_max` (default 50%, `[cpu] compile_cpu_max` in `resources.toml`); the rest wait in the queue and `mason resources` says why ("compile waiting: CPU 71% > 50%"). See `docs/decisions/0022-self-cap-readings.md` and `0026-limits-readings.md`.

## A rollback

If a new build crashes three times in five minutes, the daemon rolls back (Stage 16). That also starts a **cooldown** (`cooldown_h`, default 24) during which no self-task may be filed, and files **one proposal**, waiting for you, with what is known about the failure.

## Privacy

Self-proposals stay in the local tracker. Nothing about a self-task, and nothing from another project, is published anywhere by this.

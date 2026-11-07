# MASON working on MASON

MASON can land its own changes. This page says what that means, what stops for you, and how to turn it on. **It is off until you register the repository as a project**: nothing in the repository does that.

## Turning it on

Copy `docs/examples/mason-self.project.toml` to `<config dir>/projects/mason.toml`, point `path` at your checkout, and start (or restart) `masond`. The `[self]` section is what makes the project MASON's own. The repository's `.mason/mason.toml` is its pipeline and `.mason/landing.toml` how it lands (a batch at a time, straight to `develop`, after `cargo test --workspace`).

## What stops for you

Before a self-change may join the landing queue, the **running** daemon looks at its diff. It stops, and waits for you, if the change

- touches a **gate** (`.mason/`, `scripts/exit/`, `scripts/check-*.sh`, CI workflows, `deny.toml`, `clients/features.toml`, the golden logs), **verdict logic** (`crates/testing/`, how suites run, the self-test), a **guard** (the network guard, the step contract, webhook verification, how work is merged, batched and promoted, updates and rollback), a **budget** (resources, the scheduler, leases), **trust** (outside-text fencing, who may answer, trust sources, configuration layers, definition validation), or the **self-change rules** themselves (`protect.rs`, `selfguard.rs`); or
- **weakens a check anywhere**, even in files nobody listed: a removed assertion or test, a deleted test file, an ignored test, a raised limit (a number that went up under a name like `max`, `limit`, `timeout`, `budget`, `retries`), a lowered floor (`min_*`), or a broadened skip (`|| true`, `exit 0` in a script, `continue-on-error: true`, `skip = true`).

The scan reads only what the task adds on top of the freshest base (a base merged into the task does not count). A *new* file under `scripts/exit/` is not held by its path, and `|| true` after `kill`, `rm`, `wait`, `trap` or a probe is not a broadened skip; the hold text is one short line with file and line of each finding (decision 0019).

Everything else lands freely.

The list is compiled into the daemon. A change to the rules cannot take effect without passing the rules that are running, and the files that hold them are on the list. A change that adds a file claiming nothing is protected is held anyway, because the running rules do not read the candidate. You can **add** globs under `[self] protect = [...]` in your project file; no change can remove one.

The task shows why it waits (`mason task show N`), the alert says so, and `mason self` lists what is held. Approve with `mason task approve N`, from your own identity: anyone else is refused and the attempt is recorded. **An approval is for that exact diff**: a different diff is held again.

## Limits

Open self-tasks, tasks filed per week, and agent steps per period (the cap on spend: token cost is not recorded yet, so steps are what is counted) each have a limit in `[self]`. Filing past a limit is refused with the number and the limit, by the command line, the API and the tracker import (an issue left unimported is looked at again later). `mason self` shows where each stands. The urgent fix task the landing freeze files is not counted: restoring the base comes first.

**The weekly cap counts only work MASON starts for itself.** `per_week` counts tasks whose trust is not the owner's: agent proposals and outside issues you labeled. Tasks you file (the command line or API as the owner, the monitor, a tracker issue by an owner name) are not counted, and neither is a task cancelled before any agent step started. `mason self` reads "N of 10 this week (M of yours not counted)", and the refusal says how many of the counted are agent or outside tasks. `max_open`, `max_steps` and the cooldown apply to every filing, yours included. **An owner identity can therefore file without a weekly cap**; the guards that remain are `max_open`, `max_steps`, the cooldown and the holds on protected files (see `docs/decisions/0022-self-cap-readings.md`).

## The port

`masond` listens on **127.0.0.1:7423** unless told otherwise, so a monitor that is open when the daemon restarts (an update, a changed limit, a crash and relaunch) finds it in the same place. It listens on loopback only; nothing beyond this machine can reach it. `daemon.json` in the data directory (`pid`, `port`, `started_ms`) is still written after the bind and is what the command line and the monitors read, so a different port works too.

Change it, in order of precedence:
- `masond --port N` (`--port 0` asks the OS for a free port every start, as the tests and `--demo` do);
- `[daemon] port = N` in `daemon.toml` in the config directory (`~/.config/mason/daemon.toml`):
  ```toml
  [daemon]
  port = 7500
  ```
  An invalid `daemon.toml` stops the daemon from starting and names the file.

**When the port is taken** by another program, `masond` exits with status 1 and says which port, who holds it (the pid and program, when `lsof` can tell) and how to change it. It never picks another port by itself. Stop the other program, or set a different port as above. A second `masond` on the same data directory is refused before this (`a daemon is already running here (pid N)`). `mason update`'s self-test binds no port, so it never meets the live daemon's.

The monitors (SwiftUI and Compose) and the command line read `daemon.json` again when a connection fails. A monitor shows "Daemon restarting, reconnecting…" and retries after 1, 2, 4 and 8 seconds, then every 10, and resumes from the last event it saw; it also notices a changed `pid` or `started_ms` while connected, and no message or reply for 15 seconds counts as a stall. The command line reads the file once more after a refused connection. See `docs/decisions/0024-fixed-port-readings.md`.

## A rollback

If a new build crashes three times in five minutes, the daemon rolls back (Stage 16). That also starts a **cooldown** (`cooldown_h`, default 24) during which no self-task may be filed, and files **one proposal**, waiting for you, with what is known about the failure.

## Privacy

Self-proposals stay in the local tracker. Nothing about a self-task, and nothing from another project, is published anywhere by this.

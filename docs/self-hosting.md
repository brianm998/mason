# MASON working on MASON

MASON can land its own changes. This page says what that means, what stops for you, and how to turn it on. **It is off until you register the repository as a project**: nothing in the repository does that.

## Turning it on

Copy `docs/examples/mason-self.project.toml` to `<config dir>/projects/mason.toml`, point `path` at your checkout, and start (or restart) `masond`. The `[self]` section is what makes the project MASON's own. The repository's `.mason/mason.toml` is its pipeline and `.mason/landing.toml` how it lands (a batch at a time, straight to `develop`, after `cargo test --workspace`).

## Importing issues

The example has **no tracker section**, so the poller never runs for the project: an issue filed on GitHub does not become a task, and nothing but a note says so. `mason doctor` prints `issues on <owner/repo> are not imported; see docs/self-hosting.md` for a project whose repository has a GitHub remote and no `[tracker]`; `mason task refresh N` says the same with the config path; the monitors' landing page has a tracker line per project (off, polling, or stopped with the reason).

To turn it on, uncomment the `[tracker]` block at the end of `docs/examples/mason-self.project.toml` in your project file (`<config dir>/projects/mason.toml`), set `repo`, and restart `masond`. The token is given as a **reference**, `env:NAME` or `file:/path`, never the token itself; a literal token is refused.

- **Token scope:** read access to the repository's **issues** is all that polling needs (a fine-grained token with *Issues: read*; a classic token with `public_repo`, or `repo` for a private repository). Add **write** (*Issues: read and write*) only if you want MASON to mirror comments and closes back to the issue when the task lands or fails; without it, mirroring fails quietly and the work is not held up.
- **Outside issues need the label.** An issue by one of your owner names is imported as it is. Anyone else's is ignored until someone with triage rights puts the project's `label` (default `mason`) on it, and it is then filed with outside trust. Pull requests are never imported.
- **First poll:** every open owner issue is imported in one batch. Items over a `[self]` limit (`max_open`, `per_week`) stay unimported and are tried again at each poll; that is intended. `mason doctor`, the landing page and an alert say how many issues wait for a slot and which.
- **Order:** turn it on only when task dependencies are honoured (they are, since Stage 42): `Depends on #N` lines and GitHub's blocked-by links make issues meant to run in a chain run in order.
- **When it stops:** a permanent error (a 401 or 404: a wrong token or repository) stops the poll and raises an alert with the reason. Fix it, then `mason task refresh N` on any task of the project asks once more.

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

**`max_open` counts only tasks doing or about to do work.** Running, queued-and-claimable, paused and waiting-on-owner tasks count. A task held behind unlanded blockers, and a proposed task, spend nothing and are not counted; a held task joins the count when its blockers land and is never refused then. `mason self` reads "2/3 open (4 held behind others, not counted)" (see `docs/decisions/0032-self-open-readings.md`).

**The weekly cap counts only work MASON starts for itself.** `per_week` counts tasks whose trust is not the owner's: agent proposals and outside issues you labeled. Tasks you file (the command line or API as the owner, the monitor, a tracker issue by an owner name) are not counted, and neither is a task cancelled before any agent step started. `mason self` reads "N of 10 this week (M of yours not counted)", and the refusal says how many of the counted are agent or outside tasks. **`max_steps` counts the same way:** only steps of tasks not filed by the owner; `mason self` reads "N of 100 agent steps in 24 h (M of yours not counted)" (decision 0030). `max_open` and the cooldown apply to every filing, yours included. **An owner identity can therefore file without a weekly cap**; the guards that remain are `max_open`, the agent-filed share of `max_steps`, `per_week`, the cooldown and the holds on protected files (see `docs/decisions/0022-self-cap-readings.md`).

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

The monitors (SwiftUI and Compose) and the command line read `daemon.json` again when a connection fails. A monitor shows "Daemon restarting, reconnecting…" and retries after 1, 2, 4 and 8 seconds, then every 10, and resumes from the last event it saw; it also notices a changed `pid` or `started_ms` while connected, and no message or reply for 15 seconds counts as a stall. The command line reads the file once more after a refused connection. See `docs/decisions/0029-fixed-port-readings.md`.

## A rollback

If a new build crashes three times in five minutes, the daemon rolls back (Stage 16). That also starts a **cooldown** (`cooldown_h`, default 24) during which no self-task may be filed, and files **one proposal**, waiting for you, with what is known about the failure.

## Privacy

Self-proposals stay in the local tracker. Nothing about a self-task, and nothing from another project, is published anywhere by this.

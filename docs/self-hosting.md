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

**The weekly cap counts only work MASON starts for itself.** `per_week` counts tasks whose trust is not the owner's: agent proposals and outside issues you labeled. Tasks you file (the command line or API as the owner, the monitor, a tracker issue by an owner name) are not counted, and neither is a task cancelled before any agent step started. `mason self` reads "N of 10 this week (M of yours not counted)", and the refusal says how many of the counted are agent or outside tasks. **Limits are for resources and for work MASON starts for itself, not for how many tasks you file.** `max_open` and `max_steps` count and refuse only tasks that are not yours, like `per_week` (decision 0026). `mason self` shows how many of yours are open ("N of yours open, not limited"). The cooldown after a rollback still stops every filing, yours included, and the holds on protected files stay. **An owner identity can therefore file any number of tasks.** What bounds your own work is the machine: at most `llm:claude` agent sessions run at once (default 10; set `[llm] claude` in `resources.toml` or `mason resources limit llm:claude N`, which survives a restart), and while a local compile runs, a further one starts only while CPU use is below `compile_cpu_max` (default 50%, `[cpu] compile_cpu_max` in `resources.toml`); the first always starts, because CPU use is the whole machine's and another pipeline's builds must not hold every MASON compile back; the rest wait in the queue and `mason resources` says why ("compile waiting: CPU 71% > 50%"). See `docs/decisions/0022-self-cap-readings.md` and `0026-limits-readings.md`.

**CPU use alone does not protect the machine.** A test run spends most of its time on disk and process launches, and work blocked on a stalled disk reads as idle, so the CPU gate admits more compiles exactly when the disk can take no more. Two more limits cover that. At most `compile_jobs` compiles and test runs go at once (default 3, `[cpu] compile_jobs` in `resources.toml`, 0 for no count), until you set a count live: `mason resources limit compile N` or the stepper in the Compilation section of the monitor's home page, which replaces `compile_jobs`, survives a restart and still leaves the CPU and disk gates in place. That section lists what is compiling, what waits, and why each one waits. **A landing is not held by the CPU.** A batch's verification unblocks every task in it, so it goes ahead of all other pipeline work and starts whatever CPU use reads and however recently another compile started; it still needs a free slot, enough disk and memory, and a build disk that answers. **Order by priority first if you want urgent work to jump.** By default a task nearer landing goes first and tracker priority breaks ties; `[order] priority_first = true` in `resources.toml` puts priority first (P0 before P2 however far along), for every project at once, since one queue serves them all. A person's command and a landing keep their places. **A hung command loses its slot.** Every 30 seconds the daemon reads the CPU time of each compile holder's processes. One that used under 1% of a core since it was last seen busy shows "no CPU for N min" on the home page, and a task's command that stays that way for `[cpu] compile_idle_kill_s` (default 1800, 0 never) is stopped, which frees its slot and says so on the home page. A person's own `mason run` is only shown, never stopped. No further one starts while the build disk is slow to create a file (`[disk] stall_ms`, default 2000, 0 for off): the daemon tries it every two seconds on the data directory and each `task_cache`, a slow answer counts for 60 seconds, and the wait reads "compile waiting: the build disk took 21.0 s to answer (limit 2.0 s)". A stall holds nothing back while no compile is running, so one that is not MASON's cannot stop the queue.

## Tracker imports start proposed

An issue the tracker imports becomes a **proposed** task, with the issue number, title, body and link on it, and waits for you: `mason task approve N` or the monitors' Approve button. Nothing an outside signal creates starts without it; that is why. Set `import_as = "queued"` under `[tracker]` in the project file in your machine config to have every imported issue start at once, as before; an issue's own text cannot set it. Dependencies still import in order, and a proposed task whose blockers have not landed shows as proposed and held behind them.

Proposed tasks spend nothing, so `max_open`, `per_week` and `max_steps` never refuse them (only the rollback cooldown does); the limits apply when you approve and the task starts. When a limit does keep a queued import out, the alert reads "5 issues not imported (#...): 10 self-tasks are open and the limit is 10", `mason task list --pending-issues` and both monitors list the issues with the reason, and it clears when they import.

A task titled `... (GitHub #N)` that no issue is linked to is adopted by issue #N instead of being imported a second time. If an open issue is linked to a cancelled task, the alert says so; link a live task with `mason task relink <id> --issue N` (owner only).

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

## The watchdog

An open monitor is the only thing inside MASON that can say the daemon is down, and nothing inside the daemon can say it when it is dead or wedged (launchd restarts a dead `masond`, not a hung one). `scripts/mason-watchdog.sh` runs outside it, once a minute, using only `/bin/bash`, `/usr/bin/curl` and `/usr/bin/say`, so it still runs while `syspolicyd` is down.

Each run reads the port from `daemon.json` and asks `GET /status` with a 20-second limit (`/status` goes through the worker pool and the store's writer, so a wedged daemon does not answer). Two failures in a row is **down**: it speaks with `say` and sends through `[ntfy]` in `notify.toml` if there is one, repeats every 30 minutes while down, and says once when the daemon is back. A changed `pid`, or `uptime_s` under 60, on three runs in a row is **crash-looping**, said the same way. Where `say` is absent (Linux) the speech is skipped. State is kept in `mason-watchdog.state` beside the data directory.

To install on macOS (edit the script path first):

```
cp docs/examples/com.example.mason-watchdog.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.example.mason-watchdog.plist
```

To remove: `launchctl bootout gui/$(id -u)/com.example.mason-watchdog`. `mason doctor` notes when no agent running the watchdog is installed. The script reads `MASON_DATA_DIR`, `MASON_CONFIG_DIR` and a few `MASON_WATCHDOG_*` variables (listed at the top of the script) for a non-default setup.

## A rollback

If a new build crashes three times in five minutes, the daemon rolls back (Stage 16). That also starts a **cooldown** (`cooldown_h`, default 24) during which no self-task may be filed, and files **one proposal**, waiting for you, with what is known about the failure.

## Privacy

Self-proposals stay in the local tracker. Nothing about a self-task, and nothing from another project, is published anywhere by this. Without write access (`[self] access = "fork"`), `mason proposal publish N --preview` shows the redacted text and `--confirm` sends it upstream; client code is never attached (decision 0042).

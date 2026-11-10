# MASON

**Multi-agent Automation and Session Orchestration Network.** A Rust daemon
(`masond`) and CLI (`mason`) that run coding agents through a configurable
pipeline of exploring, developing, testing, review and landing, for any number
of projects at once, with SwiftUI and Compose monitors.

<!-- status:begin -->
Status: **pre-alpha**. What each stage note says it built (generated from `docs/stages` by `scripts/gen-status.sh` when a change lands; do not edit between the markers):

- **s00**: Stage 0: Repository and ground rules
- **s01**: Stage 1: Spikes
- **s02**: Stage 2: Core domain and reducer
- **s03**: Stage 3: Event store
- **s04**: Stage 4: Step host
- **s05**: Stage 5: Agent runner and step contract
- **s06**: Stage 6: Walking skeleton
- **s07**: Stage 7: Definitions and configuration
- **s08**: Stage 8: Scheduler and resources
- **s09**: Stage 9: Testing system (completes M2)
- **s10**: Stage 10: Attention and overrides
- **s11**: Stage 11: Protocol, views and parity tooling
- **s12**: Stage 12: SwiftUI monitor (owner checkpoint)
- **s13**: Stage 13: Compose monitor (completes M3)
- **s14**: Stage 14: Landing engine, direct and local-only
- **s15**: Stage 15: GitHub, CI and PR landing
- **s16**: Stage 16: Self-reload and updates
- **s17**: Stage 17: Self-hosting and gate protection (completes M4)
- **s18**: Stage 18: Graph editor
- **s19**: Stage 19: Plugins and extensions
- **s20**: Stage 20: Second adapters
- **s21**: Agents file proposals with duplicate rejection, required evidence and per-task and per-day caps; policy decides proposed or queued, and routing sends each to the project or to MASON. The owner approves, rejects and edits in the CLI and both clients, and the "Improve the pipeline" box runs on a branch (a fork's, without write access) with a redacted, confirm-only upstream publish.
- **s22**: MASON now notices its own problems and checks its own fixes: daily rule-based detectors file findings as proposals with evidence and a named metric, the next cycle after a fix lands reports "this fix worked" or "this fix didn't help" with before and after values, a digest and a Health page show only recorded data, recurring jobs (including base-health that files only new failures) run from the local overlay, and an optional triage records suggestions for the owner's review without ever answering.
- **s23**: Alerts now reach the owner without becoming noise: a routing table with Part 19's defaults sends one notice per channel and one reminder, then stays silent once the condition clears or is acknowledged; monitors queue a sound then one sentence, never overlapping, with one device speaking and mute leaving every visual alert; a lost daemon connection is raised by the monitor itself; and email (SMTP), ntfy and a signed webhook each pass a conformance kit.
- **s24**: A phone can now be paired with MASON over the LAN: an explicit `[lan]` setting (off by default, loopback unchanged) starts a separate TLS listener that serves only paired devices, with single-use 120 s pairing codes, per-device hashed tokens, owner and viewer roles, immediate revocation, a pinned self-signed certificate and an `_mason._tcp` mDNS record; the Swift and Compose cores gain pairing, pinning, discovery parsing and the phone layout rules, and `s24.sh` checks the daemon side end to end with a fake phone. No real phone, simulator, emulator or Wi-Fi was used.
- **s25**: Projects can declare iOS simulator, Android emulator, iPhone and Android phone environments; each suite leases one device by capability from a pool sized by a device provider (the scheduler's own wait when none is connected, rewritten as "environment problem, not the task's fault: waiting for an iPhone ..." with an `environment` alert), sees it as `MASON_DEVICE`, goes through setup, health, suite and teardown, and expired certificates and offline devices are classified environmental; `s25.sh` checks all of this on a fake provider, and no real simulator, emulator or phone was used.
- **s26**: `mason init` now takes a project from nothing to a merged `mason/setup` branch and a landed first task: the eight Part 21 questions each have a flag (`mason init --workflow solo --yes` runs without a terminal), five MIT example workflows live in `templates/`, suites are proposed and run once with a baseline recorded, the "Filing work" notes are written once into AGENTS.md or CLAUDE.md, `mason adopt`, `mason remove` and a `mason`-on-PATH check in `mason doctor` exist, and `scripts/install.sh` verifies a SHA-256 and names a clashing `mason`; `s26.sh` initialises every template on Rust, Python and Swift samples and lands a harmless task with fake-claude on each, and no real install, release, signature, tap or publish was done.
- **s40**: s40: Monitors answer and approve by clicking
- **s41**: s41: The self-hold scan flags only what a task really weakens
- **s42**: Task dependencies (s42): Task dependencies (GitHub issue #7)
- **s43**: MASON reserves each task's stage note, exit script and decision numbers when it is claimed, tells the task in its prompt, sends back a task that uses another number, and builds the README status block itself.
- **s44**: `per_week` in `[self]` counts agent and outside tasks only; the owner's own filings and tasks cancelled before any step started are not counted, and `mason self` shows the split.
- **s45**: every running step keeps a live progress record in the daemon (what it counts, done, total, last output, when it last moved), fed from its step host's output as it arrives, pushed to monitors as `StepProgress` (at most once a second for output alone) and carried by the views.
- **s46**: Both monitors show a task's plan as readable text (summary, numbered steps, files, criteria) parsed by the daemon into PlanView, with Copy plan and Copy JSON.
- **s47**: double-clicking a non-zero count tile reveals its sidebar section and, if it holds exactly one task, opens that task. Single click is unchanged.
- **s48**: no count caps the owner's tasks. The only strict limits are concurrent LLM sessions (`llm:claude`, default 10) and local compile CPU (a compile starts only while CPU use is below `compile_cpu_max`, default 50%). `max_open`, `per_week`, `max_steps` and the cooldown bound only work MASON starts for itself.
- **s49**: both monitors and the CLI show each running step's live progress: a percentage only from a real count, the count and the typical range from history, the count alone otherwise, and always how long ago the step last moved, counting up on the monitor's clock between pushes and turning amber ("no output for 6 min") when it is quiet past its usual gap.
- **s50**: The needs_attention tile reads "stuck" (label now sent by the daemon) and count tiles are sized to their label.
- **s51**: masond keeps a constant loopback port (default 7423, configurable) and the CLI and both monitors re-read daemon.json and reconnect by themselves after a restart.
- **s52**: `max_steps` counts only agent steps of tasks whose trust is not Owner, by the same predicate as `per_week`; `mason self` shows "N of M agent steps in H h (K of yours not counted)".
- **s53**: Edits to the scripts the daemon runs when changes land (`scripts/gen-*.sh`, today `gen-status.sh`) are held for the owner like the other gates.
- **s54**: the `max_open` self-hosting limit now counts only tasks doing or about to do work (running, queued and claimable, paused, waiting on the owner). Tasks held behind unlanded blockers, and proposed tasks, are not counted; `mason self` shows the held count.
- **s55**: Fixed red CI on develop: s17.sh now expects the new hold wording and Compose Editor.kt captures maxWidth in the constraints scope.
- **s56**: LLM provider accounts are real configuration (`providers.toml`: adapter, billing, sessions, warn_at, monthly_budget_usd, overage); a project names its account and its agent steps take that account's sessions, and the usage view, ProviderStatus, both monitors and `mason doctor` list every account.
- **s57**: pipelines say `deep` or `light` and `low`, `medium` or `high`; each provider's built-in table, `providers/<provider>.toml` and the project file's `[provider.<provider>]` say which real model and effort that is, for Claude and Codex alike (`fast` still means `light`).
- **s60**: A base conflict no longer fails a task: the develop agent is told to merge and resolve it. `run_check` and landing run the test command as layered (runtime overrides included), and a develop step's tool timeout covers a whole test run. The tests that failed only on a busy machine now check what they mean.
- **s61**: A test step folds the base in before it runs, so fixes that reached the base while a task waited count. A red test command can be run again (`test_reruns`), so a flaky test is a recorded flake, not a red round. A step is told what it holds (`MASON_HELD`).
- **s62**: Every agent adapter reports usage as one `UsageSnapshot` (limit windows with utilization and reset, overage, whether work is blocked, API spend), read from the stream and never estimated, and the runner keeps the latest one.
- **s63**: every place a model is shown (task row, home's active models, each run in the task detail, the editor's inspector, `mason task show` and `mason status`, with `--json`) now carries one daemon-made string such as `deep (Opus medium)`, built from what each step run actually launched, so a retry, an override and a restarted daemon all show the truth.
- **s64**: MASON now keeps each provider account's latest limit windows (across restarts), a bounded history of window readings, and per-attempt tokens and cost by project, task, step and model, and shows them with their age in both monitors, the top bar and `mason usage`.
- **s65**: a tracker import now starts as a proposed task waiting for the owner's approval (`[tracker] import_as = "queued"` restores the old behaviour), proposed filings are never refused by a self-hosting limit, an issue a limit kept out raises an alert and shows in `mason task list --pending-issues` and both monitors, and a hand-filed task titled `(GitHub #N)` is adopted instead of duplicated.
- **s66**: a step host that disappears is now explained (pids, whether they still exist, the signal, how the journal ended, the time) in the step's failure text and `mason task show`, a lost test/command/suite run is re-run once on a fresh host without counting as a failed attempt, two or more hosts lost together raise one alert, and tests and exit scripts only kill hosts they can verify they started.
- **s67**: A usage limit, outage, plan limit under `overage = "hold"` or reached api budget now holds the whole provider account (kept in the store across restarts) until the reset the provider gave or a backoff probe succeeds, tells the owner once per event, warns before a window's limit, and the owner can hold or release an account from the CLI and protocol.
- **s68**: A project with a GitHub remote and no `[tracker]` section is now noted by `mason doctor`, `mason task refresh` and the monitors' landing page, instead of silently importing nothing; the example project file has a ready-to-fill tracker block, and the poll's last result, counts, stop reason and the issues waiting for a `[self]` slot are shown.
- **s69**: A send-back for a conflict with the base (at the test step, a suite step, or landing's batch) no longer spends one of a step's `max_rounds`; it has its own limit of five. Rounds count from the owner's last retry, so a retried task gets them again.
- **s70**: Test steps count passed tests as the output arrives (against the last green run's count, else the suite's minimum), suites and script steps can append JSON progress lines (`progress`, `MASON_PROGRESS_FILE`), plugins on protocol 2 send `step.progress`, and approval steps say what they wait on.
- **s71**: Agent steps now show real progress while they run: the counts the agent gives with `report_progress` (pushed at once), its own todo list as "plan steps", and its tool calls with a one-line, redacted description of the latest; one rule picks the figure.
- **s72**: CI on `develop` was red from s60 on: tests that merged without a git identity, a demo banner whose clock text differed between two demo runs, an exit script that ended itself on `wait`, and rustls versions with advisories. The broker now counts machine slots held by processes it has no lease for, so it no longer grants them again.
- **s73**: The daemon now sorts the monitor's left panel by purpose: Queued shows the real admission order (the same rank `next_admission` uses, tasks that can go before held ones), and every other section shows the task that entered it most recently on top, using a new `Task.state_entered_at` set in one place in the reducer.
- **s75**: The owner can now free a task held by a cancelled or failed blocker, add blockers to one that has not started, and revive a cancelled task, with `mason task unblock | block | revive` and matching buttons in both monitors; for tracker-imported tasks GitHub stays the source of truth, with the owner's release kept until the relationship on GitHub changes.
- **s76**: A running agent step's cell now carries the tokens its LLM session has used so far (Claude from `--include-partial-messages`, Codex at turn end), summed over resumed and retried invocations, and both monitors show it as "12.2k in, 988 out" with the full figures on the task page.
- **s77**: A `mason run --needs` made by a task's agent is now ranked as that task's pipeline work (its steps remaining and priority) instead of as an interactive caller, so a task already in `test` is granted a `compile` slot ahead of an agent still developing; a `mason run` with no task stays interactive.
- **s78**: A project can name a task's build directory with `task_cache`; it is removed when the task lands (off the event loop, outside the repo lock, and only if the path passes the safety rules), and the reconcile sweep removes the ones a restart or a miss left.
- **s79**: Both monitors let the owner drag the divider between the left panel and the main view (220 to 640 before text zoom, the main view keeps 360, remembered per device), and Home and search stay pinned above the scrolling list.
- **s80**: The two holds tests now express the usage-limit reset relative to when the fake agent emits it, so a loaded machine can no longer make it stale.
- **s81**: Every git call, build, resolver, status script and `top` now runs in its own process group under a deadline, fetches have a lock of their own so a stalled remote cannot hold up worktrees, merges or cleanup, startup does no network, and a CPU reading over 30 seconds old counts as unknown.
- **s82**: When the base moves while a batch lands, the re-verification now runs with the repository lock released and holds the broker leases the test step or suites use, which also removes a self-deadlock when a landing suite is red on both runs.
- **s83**: an agent step whose step host is lost mid-session, or never comes up, is recorded as a host loss and continued (resumed with a note when the journal gave a session id, otherwise started again) instead of failing the task, at most twice per step run and counted from the invocation directories so a daemon restart does not reset it; a third loss fails the step and names all three.
- **s84**: A step's time limit now counts from when its host started (`started_ms` in `host.kv`), so re-attaching after a daemon restart gives a run only what is left of its limit, and a limit already past stops it at once as a timeout.
- **s85**: Below a free-disk floor (`[disk] floor_gb`, default 20) nothing new compiles or opens an LLM session, `mason resources` says why, an alert names the free space and what takes room, and `task_cache` ages out cancelled and failed tasks' directories after 7 days.
- **s86**: A panic on a background loop, a landing batch, `admit` or the store writer is now contained (loops go on and file a `thread_panic` bug, a batch fails its tasks and frees its slot, the writer ends the process so launchd restarts it), and `mason status` and `mason doctor` show it.
- **s87**: The one-daemon-per-data-directory guard is now a kernel `flock` on `daemon.lock`, and a start that declines to run (a daemon already there, a port taken) is logged as `refused` in `boots.log` and not counted as a crash by the rollback logic.
- **s88**: Alerts and notifications now run on their own thread that never waits on git or the repository lock, daemon speech runs off it with a per-sentence time limit, and a "git work is stuck" alert names what holds the repository lock.
- **s89**: MASON's own landing batch window is now 30 seconds instead of 10 minutes, so a task no longer waits ten minutes to land for a batch that saves almost nothing.
- **s91**: Logs and results files are read lossily (invalid UTF-8 becomes U+FFFD), log tails read only the last megabyte, and the hosted adapter's stderr buffer is cut on a character boundary.
- **s92**: Replay applies events in place, `Store::world()` returns a shared `Arc<World>` snapshot, appends mutate the writer's world without copying it, `verify()` uses its own connection, and `open_run` is an index, so restart time grows linearly with the log instead of with its square.
- **s93**: `run_check` and landing's re-test now run as step hosts (`t<task>r<run>-check<n>`, `b<batch>-retest<n>`), so a daemon restart finds the run again, a second call attaches to it, and the limit is enforced from the host's start.
- **s94**: Tasks #41, #42 and #43 failed with `Spawn(TimedOut, "step host did not start")` from the old 10 s/60 s start deadline; commit 536a832 already made the launcher wait up to 60 minutes and kill a host it gives up on, so no code changed here.
- **s95**: Task #75's seven "the agent failed: the step host was lost before the agent finished" failures predate s83, which made a lost agent host a restart (at most twice, counted from the invocation directories) instead of a crash; no code change was needed, and this records the check and the one thing to watch.
- **s99**: The self-update test no longer fails because its fake agent finishes on its own: the agent now waits for a release file instead of sleeping 90 s, and a failure of the host assertion reports every host's state, exit record, pids, heartbeat and journal tail.
- **s102**: `disk`, `environment`, `blocked`, `host_lost`, `runtime`, `definition`, `tracker` and the `provider_limit`/`provider_overage`/`provider_budget` alerts are routed like `failed` (monitor, sound, speech, push after 10 minutes), both monitors speak them by default, and a test fails when a raised kind is neither routed nor named monitor-only.
- **s103**: `scripts/mason-watchdog.sh`, run every minute by a launchd agent, asks `GET /status` and says (with `say` and `[ntfy]`) when the daemon is down, hung or crash-looping, and once when it is back; `mason doctor` notes when it is not installed.
- **s112**: `wait_explained` in the scheduling tests now waits on the broker's explanation under one 90 s deadline instead of panicking after a fixed 15 s, so they no longer fail on a heavily loaded machine.
- **s113**: The plugin kit tests now run each script once, untimed, before any kit call, so a slow first exec of a fresh file (macOS syspolicyd) no longer eats the initialize timeout.
- **s114**: Sound and speech deliveries are now recorded when a monitor (or the daemon's speech command) takes the announcement, not when it is queued, so an announcement that expires unheard stays undelivered and a monitor that claims later gets one catch-up announcement per live, unacknowledged, never-heard alert.
- **s115**: A `stalled` alert is raised when tasks are outstanding and neither the event log nor any running step has moved for `[stall] after` (default 30m, 0 off), naming the likeliest reasons, routed to every channel.
<!-- status:end -->

Pre-alpha. Tasks can wait for others (`mason task add --after 3,4`, GitHub's "blocked by" and `Depends on #N` lines are read when issues are imported, in dependency order; a change on GitHub holds or releases a queued task; both monitors show "waiting for" and "blocks"), the owner answers a task's question and decides its approvals by clicking in either monitor (option buttons with the recommended one marked, an Other box that sends on Enter, arrow keys and Return; Approve with the exact diff hash shown and Reject with a reason for a proposal, an approval step or a self-hosting hold; the top bar's count opens the first task that needs you; a non-owner sees the card and no buttons), a second agent (the Codex CLI), a second build system (Jenkins) and a generic one (any trigger and status, each a command or a URL) pass the same conformance kits as the first, with agent profiles that retry up a ladder of models, plain git over ssh as a host with no features, and optional `sandbox-exec` / `bubblewrap` sandboxing; a project can add what it needs without forking (script steps with `MASON_*` variables and a JSON result, prompt packs, and process plugins in `.mason/extensions/` that speak versioned JSON-RPC over stdio in any language, add a step type with an editor form made from their `config_schema`, and run only after the owner approves their content hash), the pipeline can be edited on a canvas in both monitors (swimlanes, forms from JSON Schema, a test grid, the landing settings, a simulation, a diff and what it affects, undo and redo, apply) with the daemon keeping the TOML's comments, MASON can work on its own repository, with changes to its gates (or that weaken them) held for the owner by the running version's rules, the daemon updates itself from a channel without interrupting work (an immutable build proves itself on a copy of the database, takes over by `exec`, and rolls back by itself after three crashes), all seven landing modes (CI on candidate refs, pull requests, the host's merge queue; GitHub Issues, pull requests and Actions behind small adapter traits, tested on recorded responses and a fake host), tasks land in batches (one integration branch per batch, the suites run once over the union, a red is blamed on the task that caused it, a red post-land tier freezes landing), pipelines are defined in `.mason/mason.toml`, one scheduler orders every contended resource, test verdicts are proven from output, a task that needs you asks a short decision card, and two monitors (SwiftUI in `clients/swift`, Compose in `clients/compose`) draw the daemon's server-computed views over one WebSocket and are held in parity by a machine check (`masond --demo` plays a scripted scenario).
is in [docs/DESIGN.md](docs/DESIGN.md).

## Licence

GPL-3.0-or-later, see [LICENSE](LICENSE). A draft section 7 App Store
additional permission is in
[LICENSE-ADDITIONAL-PERMISSION](LICENSE-ADDITIONAL-PERMISSION) and is awaiting
legal review. See [docs/LICENSING.md](docs/LICENSING.md) for what the licence
does and does not cover, including generated output and templates.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every commit needs a DCO sign-off.
Report vulnerabilities privately, see [SECURITY.md](SECURITY.md).

## Layout

`crates/` Rust workspace, `clients/` monitors, `schemas/` protocol and
definition schemas, `templates/` starter workflows, `prompts/` default prompt
packs, `docs/` design, decisions, spikes and stage reports, `scripts/exit/`
per-stage exit tests.

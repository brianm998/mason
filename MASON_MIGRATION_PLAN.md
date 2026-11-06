# MASON migration plan

You are building MASON (Multi-agent Automation and Session Orchestration Network), an automated development pipeline with a Rust daemon and SwiftUI and Kotlin Compose clients. This plan covers building MASON and then moving four of the owner's personal projects onto it, in 32 stages. Do them in order.

Keep `DESIGN.md` open beside this file. It sits in the same folder. It is the approved design, and references such as "Part 7" point to it. Read all of `DESIGN.md` before Stage 0, then re-read the listed Parts at the start of each stage.

## 1. Rules for every stage

### How to work

1. **Do one stage at a time.** Start a stage only when the previous stage's exit test has passed and its report is committed. Section 3 lists the few stages that can overlap.
2. **Build only what the stage lists.** If a stage doesn't name something, it belongs to a later stage. Leave a seam only when the design asks for one, such as the `host` field on leases.
3. **Follow the design.** If the design looks wrong, is ambiguous or conflicts with what you find, stop and ask the owner. Don't choose silently. Record each answer as a short note in `docs/decisions/`.
4. **Stop at owner checkpoints.** A stage marked **Owner checkpoint** ends with a message to the owner, and you wait for the answer. Otherwise continue to the next stage once the exit test passes.
5. **Never weaken an exit test to pass it.** If a test seems impossible or wrong, ask the owner.

### Proof

6. **Every exit test is a committed script,** `scripts/exit/sNN.sh`, which exits non-zero on failure. CI runs every exit script that needs no real service, a person or a real device, so earlier stages keep passing.
7. **Report what happened.** Each stage ends with `docs/stages/sNN.md`, using the template in section 4, including the exit script's actual output. If something wasn't run, say so.
8. **Turn each bug into the smallest test that catches it,** at the lowest layer possible.
9. **Keep the full local test suite under 10 minutes.** CI enforces this from Stage 0. If a stage pushes it over, fix that within the stage.

### Safety

10. **Tests can't reach real services.** From Stage 5 onward, adapters in test mode refuse any non-loopback address. Real-service runs are separate, scheduled or manual, and clearly labelled.
11. **Real LLM runs use scratch repositories only,** and only where a stage asks for them. Everything else uses the scripted fake agent.
12. **Don't touch any existing pipeline or its repositories** until Stage 27, and then only as that stage says. Don't copy their code.
13. **Never commit secrets.** Config holds references to the OS keychain or a protected file, never values.
14. **Ask before anything outward-facing that a stage doesn't explicitly authorize:** publishing packages, creating public resources, sending messages or filing real tickets.

### Code and repository

15. **Every source file carries** `SPDX-License-Identifier: GPL-3.0-or-later`. Templates and scaffolding copied into users' projects use a permissive licence instead (Part 22).
16. **Every commit carries a DCO sign-off** (`git commit -s`).
17. **Work on a `stage/NN-short-name` branch,** push it, wait for CI, then fast-forward `main`. No PR is needed for this work. Outside contributions do come in as GitHub PRs.
18. **From Stage 17 onward, MASON builds itself.** File the remaining work as MASON tasks. Use the `explored` flag when your task already holds a complete plan.
19. **Blocking work never runs on the async event loop.** Git, network and process calls go through bounded worker pools (Part 2).
20. **The reducer is pure.** `(state, event) -> (state', effects)` does no IO and never reads the clock.

### Terms in `DESIGN.md`

- "You" means the owner, and "I" means the design's author.
- "The reference monitors" and "the earlier pipelines" are the owner's existing systems. You don't need them. The design states everything you must build. The owner will compare your work against the monitors at Stage 12.
- "The desktop-focused repo" in `DESIGN.md` is the airplane remover (Stage 27).
- The owner's mobile repository was a reference for this design and is **not** a migration target. Don't plan work around it, read it or touch it.

### Fixed names

| Thing | Name |
| --- | --- |
| Product | MASON |
| Daemon and CLI commands | `masond`, `mason` |
| Crate published to crates.io | `mason-pipeline` (`mason` and `mason-cli` are taken) |
| Per-project config directory | `.mason/`, holding `mason.toml` and `landing.toml` |
| Local overlay | `~/.config/mason/projects/<id>.toml` |
| URL scheme | `mason://` |
| mDNS service | `_mason._tcp` |
| Protocol schema | `schemas/mason.proto` |
| CI candidate refs | `mason/ci/<batch>` |
| Flag label on trackers | `mason:explored` |

## 2. Stage map

| Stage | Name | Milestone | Parts | Stops for the owner |
| --- | --- | --- | --- | --- |
| 0 | Repository and ground rules | | 2, 22 | Yes |
| 1 | Spikes | M0 | 2, 3, 6, 24 | Yes |
| 2 | Core domain and reducer | M1 | 3, 4 | |
| 3 | Event store | M1 | 2, 3 | |
| 4 | Step host | M1 | 2 | |
| 5 | Agent runner and step contract | M1 | 6 | |
| 6 | Walking skeleton | M1 exit | 1-6, 9, 11 | |
| 7 | Definitions and configuration | M2 | 4, 5 | |
| 8 | Scheduler and resources | M2 | 7 | |
| 9 | Testing system | M2 exit | 8 | |
| 10 | Attention and overrides | M3 | 10 | |
| 11 | Protocol, views and parity tooling | M3 | 16, 17 | |
| 12 | SwiftUI monitor | M3 | 17 | Yes |
| 13 | Compose monitor | M3 exit | 17 | |
| 14 | Landing engine | M4 | 9 | |
| 15 | GitHub, CI and PR landing | M4 | 3, 9, 11 | |
| 16 | Self-reload and updates | M4 | 15 | |
| 17 | Self-hosting and gate protection | M4 exit | 13, 15 | Yes |
| 18 | Graph editor | M5 | 18 | |
| 19 | Plugins and extensions | M5 | 11, 12 | |
| 20 | Second adapters | M5 exit | 6, 11 | |
| 21 | Proposals and self-improvement | M6 | 13 | |
| 22 | Introspection and recurring jobs | M6 exit | 10, 14, 23 | |
| 23 | Notifications, sound and speech | M7 | 19 | |
| 24 | Discovery, pairing and mobile | M7 | 16, 20 | |
| 25 | Device test environments | M7 | 7, 8 | |
| 26 | Installer and onboarding | M7 exit | 21 | Yes |
| 27 | Migrate the airplane remover | M8 | 24 | Yes |
| 28 | Adopt ReelVault | M8 | 21, 24 | Yes |
| 29 | Adopt djukebox | M8 | 21, 24 | Yes |
| 30 | Adopt DiskFree | M8 exit | 21, 24 | Yes |
| 31 | Version 1.0 | M9 | 22, 24 | Yes |

## 3. What can overlap

Everything is sequential except:

- Stages 18, 19 and 20 can overlap once Stage 17 is done, because each one can be filed as separate MASON tasks. Stage 18 must not depend on anything unfinished in 19, and vice versa.
- Stages 28, 29 and 30 can overlap once Stage 27 has switched, if the owner wants them to.
- Stages 23 and 25 can overlap. Stage 24 needs 23's push route, and Stage 26 needs 25 for its device-tested template.

Stages 12 and 13 do not overlap. The owner reviews Stage 12's layout before Stage 13 starts, so the Compose client copies an approved layout.

## 4. Stage report template

Write `docs/stages/sNN.md`:

```markdown
# Stage NN: <name>

## Built
<What now exists, with paths.>

## Exit test
<The exact command, and its actual output.>

## Not run
<Anything not exercised, and why. "Nothing" is a valid answer only if it's true.>

## Differences from the design
<Each difference and the owner's answer that allowed it. Otherwise "None".>

## Questions for the owner
<Anything unresolved. Otherwise "None".>
```

---

# Stages

## Stage 0: Repository and ground rules

**Goal:** an empty, correctly licensed repository whose CI enforces the rules before any product code exists.

**Read:** Parts 2 and 22, and "Settled decisions".

**Before you start, ask the owner:**
- Which GitHub account or organization holds the repository, and its name. Check that the name is free.
- For permission to create it as a public repository. Create it only after a clear yes.

**Build:**
- The workspace layout from Part 2: `crates/` (`core`, `engine`, `store`, `protocol`, `adapter-api`, `adapters`, `step-host`, `testing`, `landing`, `daemon`, `cli`), `clients/`, `schemas/`, `templates/`, `prompts/`, `docs/`, `scripts/exit/`. Empty crates compile.
- `LICENSE` containing GPL-3.0-or-later.
- A **draft** of the GPLv3 section 7 App Store additional permission in `LICENSE-ADDITIONAL-PERMISSION`, marked as awaiting legal review. Don't present your wording as legal advice.
- `README`, `CONTRIBUTING` (DCO instructions), `CODE_OF_CONDUCT`, `SECURITY` with private reporting, and the licence note saying generated output isn't covered by the GPL.
- Copy `DESIGN.md` into `docs/`. **Don't commit this plan to MASON's repository.** It names the owner's projects and machine, and it isn't useful to outside readers. Keep it where the owner gave it to you, and refer to stages by number in commits and reports.
- GitHub Actions CI on hosted macOS and Linux runners: `cargo fmt --check`, `clippy -D warnings`, `cargo test`, `cargo-deny` (advisories and GPL compatibility), SPDX header check, DCO check, a 10-minute budget check, and a runner for `scripts/exit/*.sh`.
- Fork PR safety: use `pull_request`, never `pull_request_target`, with no secrets and no self-hosted runners.
- Pin the Rust toolchain in `rust-toolchain.toml`, and commit `Cargo.lock`.

**Exit test:** `s00.sh` confirms the workspace builds and that the header and DCO checks reject a deliberately bad sample. CI is green on `main`.

**Owner checkpoint:** show the owner the additional-permission draft and ask them to get it checked. Store builds can't ship until that's done, but work can continue meanwhile.

## Stage 1: Spikes

**Goal:** written answers to the design's open technical risks, before anything depends on them.

**Read:** Parts 2, 3 and 6, and M0 in Part 24.

**Build:** throwaway code in `spikes/`, kept out of the workspace crates, and one result file per spike in `docs/spikes/`. Each result gives the method, measured numbers, a conclusion and the fallback if needed.

| Spike | Question to answer | Fallback |
| --- | --- | --- |
| Suspend and resume | Can each of Claude Code and Codex CLI end a turn on a pending long check and resume cleanly with its result? What happens to cost when the prompt cache expires during long waits? | A held session that doesn't count toward the slot limit |
| Long MCP tool calls | What timeouts apply to a long MCP tool call in each CLI, and how can they be controlled? | Always use suspend, or poll through a short tool |
| Step-host reattach | Can a detached process keep a child running, journal its output and be reattached by a new daemon after `kill -9`, on macOS and Linux? | Must not be needed. Stop and tell the owner if it fails. |
| ETag polling | Do conditional GitHub requests behave as Part 3 says, including rate-limit accounting? | Slower polling, with limits shown |

**Exit test:** `s01.sh` checks that all four result files exist and each has a conclusion. The step-host spike's `kill -9` test runs automatically.

**Owner checkpoint:** send the owner the four conclusions and any fallback you intend to use. Wait for an answer.

## Stage 2: Core domain and reducer

**Goal:** the task model and state machine as pure code.

**Read:** Parts 3 and 4.

**Build in `crates/core` and `crates/engine`:**
- The entities: Project, Task, StepRun, Wait, Decision, Verdict, Batch, Proposal, Alert, Override, Debt.
- The eleven states: proposed, queued, exploring, developing, testing, review, landing, needs attention, failed, canceled, and landed with its outcome of merged or closed with no change.
- The activity field, separate from state: running, waiting(on), held(by), paused. Exactly one of each is true at any time, and they change together.
- Events, effects and the pure reducer. Time arrives in events, so tests use a simulated clock.
- Transitions from Part 4's diagram, including the `queued` to `developing` route and the `developing` to `exploring` route.
- Blockers that release themselves when the blocker lands, not-before times, retry to any step, and cancel from any non-terminal state.

**Tests:** every transition, including illegal ones, with no IO.

**Exit test:** `s02.sh` runs the reducer tests and confirms `crates/core` and the reducer have no IO or clock dependencies.

**Not in this stage:** storage, processes, graphs loaded from files.

## Stage 3: Event store

**Goal:** a durable append-only log with projections.

**Read:** Parts 2 and 3.

**Build in `crates/store`:**
- SQLite through `rusqlite`, in WAL mode, behind a single writer thread.
- An append-only event log with sequence numbers, plus projections rebuilt from it.
- Migrations, and a test that runs them against a copy of an older database.
- Read by sequence number, so a client can resume from its last one.

**Tests:** rebuilding projections from the log gives the same state as live updates. A crash during a write leaves a consistent database.

**Exit test:** `s03.sh` appends events, kills the writer, reopens the store and confirms that nothing committed is lost and nothing is half-applied.

## Stage 4: Step host

**Goal:** a running step survives the daemon's death. This is the design's most important decision.

**Read:** Part 2, and your Stage 1 result.

**Build in `crates/step-host`:**
- A small detached binary, in its own process session, that owns one child process, journals its output to a file and talks to the daemon over a unix socket.
- A registry the daemon uses to find running hosts after a restart.
- Reattach: the new daemon connects, receives the buffered output and continues.
- Current and previous protocol versions are both accepted.
- Heartbeats for leases.

**Tests:** crash the daemon during output, during child exit and during a socket write.

**Exit test:** `s04.sh` starts a long-running command under a host, sends `kill -9` to the controlling process, starts a new one and confirms it reattaches, loses no output and sees the exit status.

## Stage 5: Agent runner and step contract

**Goal:** an LLM session runs behind one interface and can end only through validated tools.

**Read:** Part 6, and your Stage 1 results.

**Build:**
- The `AgentAdapter` trait and normalized `AgentEvent` and `Outcome` types.
- The scripted fake agent, which speaks real CLI formats captured from real runs and stored as fixtures.
- The Claude Code adapter.
- The contract tools over MCP, through the step host: `submit_plan`, `submit_change`, `submit_review`, `ask`, `run_check`, `report_progress`, `file_proposal` and `request_exploration`. A submit tool replies "accepted" or precisely what's wrong.
- Nudging when a turn ends with no accepted submission, then failing the step after a set number of nudges.
- Suspend during long checks, or the fallback chosen in Stage 1.
- The runner's controls: sanitized environment, explicit tool timeouts, the no-human-is-watching preamble, no fetch or push by agents, read-only tool sets for read-only steps, and a resume note instead of replaying the original prompt.
- Token attribution per session and step, showing only what the stream honestly reports.
- Provider holds: a usage limit or outage is a wait, never a failure.
- The test-mode rule: adapters refuse non-loopback addresses.
- The conformance kit for `AgentAdapter`. Run it against the fake and the Claude adapter with recorded fixtures.

**Exit test:** `s05.sh` runs the fake agent through accepted, rejected-then-fixed, no-result, timeout, usage-limit and crash scenarios, and confirms a real-address call fails in test mode.

## Stage 6: Walking skeleton

**Goal:** one real task goes from queued to landed. This completes M1.

**Read:** Parts 1 to 6, the `direct` mode in Part 9, and the plain git paragraph in Part 11.

**Build:**
- `masond` and `mason`. A fixed graph in code: explore, develop, one test command, `direct` landing.
- The local tracker, which needs no service.
- Plain git operations through the `git` CLI: per-task worktrees, branches, and a simple merge and push. The daemon does all remote git work.
- `tokio` and `axum` for the daemon, with bounded worker pools and an event-loop stall watchdog.
- CLI commands: `mason task add`, `mason task list`, `mason task show`, `mason task cancel`, `mason task retry`, `mason logs`, `mason status`, `mason doctor` (basic).
- The reconcile sweep. If it finds something the event path missed, it fixes it and records a bug.
- The crash-injection harness: kill at every effect boundary, restart, and confirm nothing is lost or done twice. Add event replay tests from recorded runs, and fake-agent end-to-end tests.

**Exit test:** `s06.sh` uses the fake agent to land a task on a scratch repository, sending `kill -9` to the daemon mid-session and confirming nothing is lost. A separate manual script, `s06-real.sh`, lands one real task on a scratch repository with Claude Code. Run it once and include the output in the report.

## Stage 7: Definitions and configuration

**Goal:** the fixed graph becomes a typed, versioned definition, and clear tasks skip exploring.

**Read:** Parts 4 and 5.

**Build:**
- The `.mason/mason.toml` format, schema-versioned, read and written with `toml_edit` so comments and ordering survive.
- Step types `agent`, `command`, `test`, `gate`, `approval`, `ci`, `pr` and `landing`. Implement `agent`, `command` and `approval` fully now. Other stages finish the rest.
- The graph validator: every path reaches a terminal state, every loop has a bound, fan-out and joins behave, and all references exist.
- The dry run, which walks scripted tasks through a graph with no LLM or network.
- The five configuration layers, in the design's order. Runtime overrides carry an owner and expiry.
- The base-branch rule: the definition comes from the base branch, and each task stays pinned to the definition it started under.
- Live reload of valid definitions. An invalid one is refused and the current one keeps running.
- **Skipping exploration (Part 4):**
  - `explored_entry` in the definition, checked by the validator.
  - The flag from `mason task add --explored`, the `explored` argument on `file_proposal`, `explored: true` front matter and the `mason:explored` label.
  - `mason help filing`, which documents all of the above.
  - The deterministic clarity check, which explores on any doubt. Add the optional cheap-model classifier only behind a project setting.
  - The flag counts only on tasks from the owner or the owner's agents.
  - Recording which route skipped exploring, "Explore anyway", and `request_exploration` with no failure charged.

**Tests:** property tests for validator soundness. Table tests for the clarity check. A test that outside text claiming `explored` is ignored.

**Exit test:** `s07.sh` runs two different definitions through the fake agent, confirms a flagged task goes straight to `developing` while an unflagged vague one explores, and confirms an invalid definition edit leaves the old one running.

## Stage 8: Scheduler and resources

**Goal:** one ordering rule for all contended resources across projects.

**Read:** Part 7.

**Build:**
- The rank tuple: interactive caller first, fewest steps remaining to landed, tracker priority, earliest enqueue time, task id. Steps remaining is the shortest graph path, ignoring loops.
- LLM session pools per provider account, adjustable live. Lowering the limit never kills anything.
- Machine classes held by kernel file locks, so a dead holder frees them automatically.
- Named pools with leases and step-host heartbeats.
- All-at-once acquisition, the passing rule from Part 7, and no gating on load average. Add the optional free-memory floor.
- No claim until an LLM slot is available.
- Every wait explains itself in the form "Waiting for compile slot: 2nd in line, behind #88 (2 steps from landing), held by #91 for 4 min".
- `mason run --needs compile -- <cmd>` for interactive sessions, and the optional agent hook.
- A second registered project, to prove sharing and isolation.

**Tests:** property tests with `proptest`: ordering always follows the rule, and there's no deadlock or lost lease. Waits are excluded from every timeout.

**Exit test:** `s08.sh` runs two projects at once with contended resources. It asserts the admission order against the rule, and confirms a killed lock holder frees its resource and the next waiter starts immediately.

## Stage 9: Testing system

**Goal:** test verdicts proven from output. This completes M2.

**Read:** Part 8.

**Build in `crates/testing`:**
- Suite, Environment, Level, Binding and Impact map. An unmapped path runs everything.
- Parsers: JUnit XML, TAP, xctest, Swift Testing, cargo-nextest, pytest, Gradle, Playwright, Cucumber JSON and a regex fallback.
- Green requires proof: executed count at or above the minimum, and at least one. A vacuous pass is red.
- Red classification: flaky, environmental, baseline and product. Baseline results are cached per base commit.
- Closing verdicts: `unproven`, `unverified`, `untested`, `verified`.
- Evidence stored per tree hash and suite, so identical code never tests twice.
- Force skip for any suite, step or gate, even mid-run, keeping green lanes and recording a debt. Debts can schedule a deferred run.
- The optional gates: negative control, assertion-weakening scan, acceptance table, and quarantine with an owner and expiry.
- Environments for local commands and for Docker Compose stacks. Simulators, emulators and devices come in Stage 25. Keep the environment interface general enough for them.

**Tests:** a fixture for every parser, including a vacuous pass for each.

**Exit test:** `s09.sh` proves M2: two projects at once in the correct order, a vacuous pass is caught and goes back to development, a flaky failure is classified on rerun, and a baseline failure is excused.

## Stage 10: Attention and overrides

**Goal:** questions and overrides work on the server, ready for clients.

**Read:** Part 10.

**Build:**
- `ask` enforces the decision card shape: a one-sentence ask of at most 200 characters, why it's for the owner, 2 to 4 options with exactly one recommended, and at most 10 lines of evidence.
- The rewrite for a failing card, then a deterministic fallback, so no question is ever lost.
- Answers tied to a hash of the question, so a stale answer is refused. Answers count only from the owner's identity.
- A gate failing again after an answer is recorded as acknowledged and isn't asked again.
- Overrides: cancel, skip test, land now, force, retry from step, pause (per project or global, for all work, new claims or landing), and open session. Land now and force can stay stubbed until Stage 14, but the commands and records exist.
- Alerts tied to live conditions, which clear when the condition clears.
- The "interventions per landed task" metric.

**Not in this stage:** the triage agent, which is in Stage 22.

**Exit test:** `s10.sh` confirms a bad card is rejected and rewritten, a stale answer is refused, an answer resumes the task, and skip and cancel work through the CLI with the right records.

## Stage 11: Protocol, views and parity tooling

**Goal:** everything clients need, before either client exists.

**Read:** Parts 16 and 17.

**Build:**
- `schemas/mason.proto`, with generated code through `prost`, `swift-protobuf` and Wire.
- One WebSocket carrying commands and subscriptions. A client sends its last sequence number and gets a snapshot or the missed events. Add the JSON mode.
- Server-computed views: `HeaderView`, `SidebarView`, `HomeView`, `TaskDetailView`, `LandingView`, `UsageView`, `EditorView`. Each entity carries `allowed_actions`, its "why waiting" text and its progress.
- `SidebarView` does the sorting: active states oldest first by time since enqueue, and failed, landed and canceled newest first.
- Move the CLI onto the same API, so everything a monitor can do, the CLI can do.
- Loopback binding by default. LAN access, pairing and tokens wait for Stage 24.
- `masond --demo`, playing scripted scenarios.
- `clients/features.toml` and its CI check. A feature marked done in only one client fails CI unless it has a recorded platform reason.
- The conformance runner in `clients/conformance/`, which checks visible content and allowed actions through accessibility trees.
- One design-token file, generated for both clients.

**Targets to measure:** a step ending and the next being admitted within 100 ms, and a state change reaching a local client within 250 ms.

**Exit test:** `s11.sh` connects a test client, disconnects it, resumes from its last sequence number and confirms it receives only what it missed. It also checks both latency targets and confirms the feature check fails on a planted imbalance.

## Stage 12: SwiftUI monitor

**Goal:** the macOS monitor, for the owner to judge.

**Read:** Part 17 closely. The window layout is the owner's explicit requirement.

**Build in `clients/swift/`:** a `MonitorCore` package and a macOS app.

- **Top bar on every page:** back (⌘[), forward (⌘]), home; the attention indicator, shown only while something needs a look; the project filter; LLM usage lines; one count tile per state; add task; mute.
- **Count tiles:** a large count over a one-line label, fixed size, tinted when the state wants the owner, with a tooltip. Clicking expands and scrolls to that sidebar section. Show a dash while disconnected, never a stale count or a false zero. **Counts don't appear on the home view.**
- **Banners** under the top bar, only while true.
- **Sidebar on every page:** a Home row, search with name, title and body checkboxes, a collapsible section per state in the design's order, each with its count, then links to the other pages. Collapse choices are remembered per device. Failed, landed and canceled start collapsed.
- **Sidebar rows:** key and title, project, activity, time in state, progress cells, model when running, and blocker links.
- **Main view:** exactly one of Home, task detail or another page. Selecting a row replaces Home with that task's detail, while the top bar and sidebar stay.
- **Home:** overall status, attention queue, active models with task and step, the session-limit stepper, landing queue, and per-stage detail.
- **Task detail:** step progress, activity, live log, plan, code changed, checks, tests, tokens, history, decision card with options plus "Other" and a text box, and actions. One-click copy of branch, worktree and key. Why exploring was skipped, with "Explore anyway".
- **Progress cells:** follow the legend in Part 17. No percentages. Estimates are ranges from history, shown only with enough samples. Running long turns amber, never red.
- **Navigation:** browser-style history and `mason://` deep links.
- **Text scale:** ⌘+ and ⌘− reflow, never zoom a rendered image.
- **Landing, LLM usage and Health pages:** with the data the server has so far. The Pipeline page is a placeholder until Stage 18.

The client draws and sends commands. It doesn't re-derive business rules.

**Exit test:** `s12.sh` runs the macOS client against `masond --demo` through conformance. A manual run confirms that answering a question in the monitor resumes a task on a real daemon, and the report says what was seen.

**Owner checkpoint:** send the owner screenshots of Home, an expanded sidebar, a task detail and a decision card, all from `--demo`. The owner compares them with the reference monitors. Don't start Stage 13 until they approve the layout. Record any changes in `DESIGN.md`.

## Stage 13: Compose monitor

**Goal:** the same monitor in Kotlin Compose. This completes M3.

**Read:** Part 17, and the layout approved in Stage 12.

**Build in `clients/compose/`:** a shared core and a desktop app for macOS, Windows and Linux, with the same layout, behaviour and design tokens as the SwiftUI client. Every feature in `features.toml` is done in both, or has a recorded platform reason.

**Exit test:** `s13.sh` runs the same conformance scenarios against both clients and the feature check passes. Answering from either monitor resumes a task, and cancel and skip work from both.

## Stage 14: Landing engine

**Goal:** batched, crash-safe landing using only local git.

**Read:** Part 9.

**Build in `crates/landing`:**
- `landing.toml`: mode, base, window, `max_size`, `parallel`, tiers and the red ladder.
- The sequence in Part 9: the task frees its slots and joins the queue, a batch forms by rank, branches merge into a disposable integration branch in a persistent worktree, suites run once over the union diff, then promote under a lock.
- Conflict handling: a resolving agent, then ejecting only that task.
- The red ladder: rerun failures, blame by impact, bisect by halves, then ask. Only blamed tasks are parked.
- Folding in a moved base and re-verifying, up to a limit.
- Parallel batches when files and suites don't overlap. Report queue idle time.
- Post-land tiers, and the freeze that lets only fixes through and files a top-priority fix task.
- Land now and force, each leaving a debt.
- Modes in this stage: `direct` and `local-only`. They refuse a tier that names `ci`, and the error points to the CI version.
- The Landing page in both clients.

**Tests:** crash injection at every landing phase. Merges are idempotent, and a crash never loses or double-lands a task.

**Exit test:** `s14.sh` lands a batch of five tasks with one conflict and one culprit. It confirms the other three land, only the culprit is parked, two disjoint batches run in parallel, and a `kill -9` at each phase recovers.

## Stage 15: GitHub, CI and PR landing

**Goal:** real hosts and CI, with and without PRs.

**Read:** Part 3 (outside services), Part 9 and Part 11.

**Build:**
- GitHub Issues tracker and GitHub host adapters, using the `TrackerAdapter` and `GitHostAdapter` traits. Capabilities are declared, and conformance kits run against recorded responses.
- **No Asana adapter.** It isn't in the first release.
- The GitHub Actions build-system adapter.
- External signals: webhook or relay, conditional polling only for items something is waiting on, with backoff, and refresh on demand. Permanent errors stop the poll and raise attention at once.
- Tracker authority rules: the tracker owns title, body, priority and closure, the database owns state, and mirroring out is best-effort and never blocks work.
- Outside issues and PRs are ignored until the owner labels them. Outside text is fenced as untrusted in prompts.
- The remaining landing modes:
  - `direct-ci`: verify locally, push `mason/ci/<batch>`, run CI on it, and on green fast-forward the base and push.
  - `local-only-ci`: the same, stopping at the local base branch. Only the candidate ref leaves the machine.
  - `pr-each`, `pr-batch` and `host-queue`.
- Candidate refs are unique per batch, never force-pushed, and deleted after promotion or ejection. CI red goes to the same ladder. A moved base folds in the new tip and re-runs CI.
- Add the check for a workflow trigger on `mason/ci/**` or `workflow_dispatch` to `mason doctor`. Stage 26 reuses it in `mason init`.

**Tests:** recorded responses only, including 404, 422, rate-limit and outage cases. A test that a permanent error stops polling at once.

**Exit test:** `s15.sh` runs each of the seven modes against a local fake git host and fake CI, including a red and a moved base in each CI mode. A manual `s15-real.sh`, using a scratch GitHub repository the owner has approved, runs `direct-ci` and `pr-batch` for real. Include its output in the report.

## Stage 16: Self-reload and updates

**Goal:** new code takes over with no interrupted work.

**Read:** Part 15.

**Build:**
- Channels `stable`, `main` and `branch:<name>`.
- Builds into `runtimes/<sha>/`, never running from a checkout that can move.
- Self-test before takeover: migrations against a copy of the database, then the golden replay of scripted tasks in a sandbox.
- Back up the database and `exec` the new binary. Step hosts keep running and the new daemon reattaches.
- Automatic rollback after three crashes within five minutes, with an alert.
- The updater refuses to move to an ancestor of the running version without an explicit rollback.
- Step hosts and clients accept the current and previous protocol versions.
- Client update banners with a button that fetches or builds the new version, launches it and closes the old one. A too-old client stays read-only and says why.

**Exit test:** `s16.sh` updates the daemon while a fake-agent session runs and confirms the session is uninterrupted. It also confirms a crashing build rolls back automatically and a downgrade without explicit rollback is refused.

## Stage 17: Self-hosting and gate protection

**Goal:** MASON lands its own changes. This completes M4.

**Read:** Parts 13 and 15.

**Build:**
- Register MASON's own repository as a project, with its own `.mason/` definition and tests.
- Self-change protection: changes touching gates, guards, budgets, self-change rules, verdict logic or trust stop for owner approval. All other self-changes land freely.
- The **running** version's rules do the check, so a change can't switch off the check on itself.
- A scan for semantic weakening in any diff: removed assertions, raised limits, a broadened skip.
- Limits on open self-tasks, tasks per week and spend per period. A rollback starts a cooldown and files a proposal.

**Exit test:** `s17.sh` confirms a planted gate-weakening change stops for approval, including one outside the protected files. A manual run has MASON land a real small change to itself and survive its own update with no interrupted session. Include the output.

**Owner checkpoint:** tell the owner that MASON can now land its own work, and ask them to approve turning it on for this repository. After approval, file the remaining stages as MASON tasks.

## Stage 18: Graph editor

**Goal:** edit a pipeline graphically in both clients.

**Read:** Part 18, and the graph rules and key decisions in Part 5.

**Build:**
- `EditorView` from the daemon, which validates and computes layout. Hand-placed positions go in a small sidecar file.
- Canvas with swimlanes by state category, a palette, success, failure and environment-failure edges, and problems shown on nodes.
- Inspector forms generated from JSON Schema.
- Create a step from scratch or with `extends`, with a prompt editor and resource picker.
- The test grid (suites by levels), and the landing editor (mode, batch rules, tiers, red ladder).
- Simulate a scripted task through a draft.
- Impact and TOML diff preview, undo and redo, then apply. GUI edits apply immediately and the daemon commits them to the base branch, or opens a PR if the project requires one.

**Exit test:** `s18.sh` drives the editor's API to add, connect, remove and edit steps, confirming that comments in the TOML survive and invalid edits are refused. Conformance runs in both clients.

## Stage 19: Plugins and extensions

**Goal:** add what a project needs without forking.

**Read:** Parts 11 and 12.

**Build:**
- Versioned JSON-RPC over stdio, with manifests. Any language works.
- A plugin can provide a step type, parser, environment or adapter.
- The four extension levels: `extends`, script steps, process plugins in `.mason/extensions/`, and prompt packs.
- `config_schema`, so a plugin's step gets a form with no client code.
- Trust: approval by content hash on first use and again on change. Extensions from task branches or outside PRs never run.
- Conformance kits usable by external implementations.

**Exit test:** `s19.sh` confirms a plugin written in Python provides a step type that appears in the editor's palette with a generated form. It runs in a graph, and changing its hash requires new approval. An extension on a task branch doesn't run.

## Stage 20: Second adapters

**Goal:** prove the interfaces are neutral. This completes M5.

**Read:** Parts 6 and 11.

**Build:** the Codex CLI agent adapter, the Jenkins build-system adapter and the generic build adapter (a trigger command or URL plus a status command or URL). Agent profiles with a retry ladder that can move up a model. Plain git as the host for a bare repository over SSH. Optional OS sandboxing with `sandbox-exec` and `bubblewrap`.

**Exit test:** `s20.sh` confirms each adapter passes its conformance kit. A custom step made entirely through the GUI API runs in a graph with the Codex fake fixtures. A task lands on a bare repository with no host features.

## Stage 21: Proposals and self-improvement

**Goal:** agents file what they notice, and MASON can improve itself on request.

**Read:** Part 13.

**Build:**
- `file_proposal` with duplicate rejection, required evidence, per-task and per-day caps, and routing to the project or to MASON.
- Policy by source and severity, deciding `proposed` or `queued`.
- Proposal rows and detail in both clients: approve, reject and edit, with evidence.
- The "Improve the pipeline" box and matching CLI command.
- With write access: a branch in the repo, landing by the configured mode.
- Without write access: a branch on the owner's fork, a local switch to channel `branch:<name>`, and an upstream PR only after the owner agrees. Rejoin the main channel when merged, and ask about carrying the patch if it's rejected or conflicts.
- Privacy: self-proposals stay in the local tracker by default. Publishing upstream takes the owner's review of a redacted version, and code from client projects is never attached.

**Exit test:** `s21.sh` confirms duplicates and evidence-free proposals are rejected. A self-improvement task without write access runs locally on its branch through a fake host. A publish attempt shows the redacted version and attaches no client code.

## Stage 22: Introspection and recurring jobs

**Goal:** MASON notices its own problems and checks its own fixes. This completes M6.

**Read:** Part 14, the triage section of Part 10, and mutation testing in Part 23.

**Build:**
- Any graph can run on a schedule, at low rank, using the same resources, monitor and alerts. Red results on the base are compared with a baseline, so only new failures file tasks.
- Rule-based detectors, daily with no LLM, for every example in Part 14.
- The analyst agent, weekly by default, writing proposals with evidence and the metric it expects to move.
- Closing the loop: the next cycle after landing reports "this fix worked" or "this fix didn't help".
- The digest, computed only from recorded data, including exploring skips by route and how many came back, plus forced landings with reasons.
- The Health page in both clients.
- The optional triage agent, with a sample of its answers queued for owner review.
- Scheduled `cargo-mutants` and scheduled live adapter runs.

**Exit test:** `s22.sh` plants a problem (a retry loop on a permanent error) in recorded history. MASON finds it, files a proposal, lands a scripted fix, and the digest reports the metric moved. A second planted fix that does nothing is reported as not helping.

## Stage 23: Notifications, sound and speech

**Goal:** alerts reach the owner without ever becoming noise.

**Read:** Part 19.

**Build:**
- The routing table of conditions and channels, with the design's defaults, and quiet hours and escalation delays per channel.
- One alert per condition, cleared when it clears, with one reminder and no loops. Acknowledging stops the noise.
- Client sound and speech: a short sound then one sentence naming the task, what's wrong and what to do. States, sounds and voice are configurable, with voices listed from the OS. "Wants a decision" and "broke" sound different.
- Announcements queue and never overlap. Mute is always visible and never hides visual alerts.
- Only one device speaks.
- Monitors raise a lost daemon connection themselves. Optional daemon speech when no monitor is open.
- Email over SMTP, ntfy push and a generic webhook, each with conformance tests.

**Exit test:** `s23.sh` confirms a condition produces one alert, one reminder, and silence after it clears. It confirms two simultaneous announcements queue, muting leaves visuals, and only one of two connected clients speaks, using a fake speech sink.

## Stage 24: Discovery, pairing and mobile

**Goal:** phones can find, pair with and use MASON.

**Read:** Parts 16 and 20.

**Build:**
- One explicit setting for LAN access. Loopback stays the default.
- mDNS `_mason._tcp` with instance name, protocol version and certificate fingerprint.
- Pairing by QR or short code from an already trusted client. Per-device tokens, pinned self-signed TLS, owner and viewer roles, and revocation.
- iOS app sharing `MonitorCore`, with the local-network permission and declared Bonjour service. Android app sharing the Compose core, using NSD.
- Phone layout: the sidebar is the first screen, with Home as its top row and count tiles scrolling sideways above it. Selecting a row pushes its detail.
- Phone features: home, task lists and detail, decision cards, cancel, skip, land now, session limit, landing queue, usage, mute and speech. Graph editing, log deep-dives and agent sessions are recorded as deliberate differences in `features.toml`.
- Catching up from the last event on opening, and background alerts through ntfy.

**Exit test:** `s24.sh` confirms an unpaired client is refused, a paired one works, a revoked token stops working, a viewer token can't change anything and a changed certificate is rejected. Conformance runs on simulators and emulators. A manual run pairs a real phone over Wi-Fi, and the report says what was seen.

## Stage 25: Device test environments

**Goal:** mobile projects can test on simulators, emulators and physical devices.

**Read:** the resources in Part 7, and environments and verdicts in Part 8.

**Build:**
- A device provider adapter with a conformance kit and a fake provider for tests.
- Environments for iOS simulators, Android emulators and physical devices, each with lease, setup, health check and teardown.
- Named device pools leased by capability, using the scheduler from Stage 8.
- A wait for a person to plug a device in, shown with what it's waiting for and charged to no task.
- Environmental red signatures for mobile, such as no device or an expired certificate, so these become a wait plus an alert and never blame the task.

**Exit test:** `s25.sh` uses the fake provider. It confirms devices are leased by capability, a missing device is a wait and not a failure, plugging one in resumes the task immediately, and an expired certificate is classified as environmental. A manual run takes a small new sample iOS and Android app through a real simulator, a real emulator and a real phone, and the report says what was seen. Use a sample app, not an existing repository.

## Stage 26: Installer and onboarding

**Goal:** a stranger gets a first task running within 10 minutes. This completes M7.

**Read:** Part 21.

**Build:**
- Install routes: Homebrew, a signed install script and `cargo install mason-pipeline`. The installer detects a clashing `mason` command, such as Dart's, and tells the user how to resolve it.
- `mason init`, following the eight questions in Part 21 in order. Every question has a flag, so `mason init --workflow solo --yes` works.
- The statement "This is only a starting point. You can change anything, any time, in the monitor or in `.mason/`." appears at the workflow step.
- The five example workflows in `templates/`.
- Suite proposal, with one run of each to confirm parsing and capture a baseline and typical times.
- Writing the "Filing work" section to `AGENTS.md` or `CLAUDE.md`, explaining the `explored` flag and when to use it.
- CI-mode workflow trigger check and offer.
- `mason doctor`, `mason adopt` and clean removal.

**Exit test:** `s26.sh` runs `mason init --yes` for each template on sample Rust, Python and Swift repositories, then lands a harmless task with the fake agent.

**Owner checkpoint:** ask the owner to try a real installation from nothing on a project of their choice, or to arrange a stranger to do it, and time it. M7's exit test is 10 minutes to a landed task.

## Stage 27: Migrate the airplane remover

**Goal:** the airplane remover (`star`) runs on MASON, and its existing pipeline, `starpipe`, is stopped but intact.

**Read:** "Migrating without risking current work" in Part 24, then in that repository: `CLAUDE.md`, `pipeline/README.md`, `pipeline/src/starpipe/adapter_star.py` and `.claude/skills/file-tickets/SKILL.md`.

**Repository:** `git@github.com:brianm998/starry_timelapse_airplane_remover.git`, base branch `develop`, public. The owner's checkout is `~/git/nighttime_timelapse_airplane_remover`. Work in a worktree and never edit the main checkout.

**Rules that apply for the whole migration. These come from the repository's `CLAUDE.md`, and a mistake here costs the owner real time:**
- **Never run the GPU median merge on this Mac.** It crashes the machine. Every suite and star run sets `useGPUForMerge = false`, and no suite relies on the default.
- **Heavy jobs need the shared lock while both pipelines run.** Parallel heavy runs have logged the owner out. During the shadow period, MASON's suites go through `pipeline/bin/starpipe heavy -- <cmd>`, so the two systems share one queue. Remove that wrapper at the switch, when MASON's machine classes take over.
- **The product's rules stay:** the Localization rule and "Swift and Kotlin clients change together" still bind every task, so put them in the project's prompt pack.
- **No PRs for internal work.** That's why `direct-ci` is the proposed landing mode below.

### Step 1: Inventory

Check this starting list against the code, and add anything it misses. `starpipe` is the running pipeline and the code is right where this table is wrong.

| What `starpipe` does | Where | In MASON | Kind |
| --- | --- | --- | --- |
| Impact map and checks, with exact expected test counts | `adapter_star.py` | Suites and impact map in `.mason/`, using minimum counts | Built-in |
| Concurrent test shards | `shards.py`, `ShardPlan` | Has no built-in equivalent. A script step that runs the shards and emits JUnit | Extension |
| Worktree artifact symlinks: opencv, StarDecisionTrees, `gui/videos`, `external_binaries/bin` | `ARTIFACT_LINKS` | A worktree setup script step | Extension |
| Heavy classes: compile, test suite, exclusive for star runs, UI and Gherkin | `heavy.py` | Machine classes | Built-in |
| Interactive hook wrapping heavy commands | `hooks/heavy_wrap.py`, `.claude/settings.json` | `mason run --needs` and the agent hook | Built-in |
| Check tiers T0 and T1 | `check` | Levels and bindings | Built-in |
| Tickets carrying a `## Plan` skip EXPLORE | `plan.py`, `tickets.py` | The `explored` flag | Built-in |
| Stage prompts: explore, develop, fix, review, resolve, CI fix, conflict | `pipeline/prompts/` | Prompt pack in `.mason/prompts/` | Extension |
| Stage fence around model tools | `guards.py` | Runner controls, plus a project prompt or sandbox rule for anything left | Built-in |
| Sonnet by default, opus only when the owner allows it | `allow-opus`, `model_<stage>` | Agent profiles, and an owner-only model override | Built-in |
| Landing train: one CI for many tickets | `train.py`, `ship.py` | Batches in `direct-ci` | Built-in |
| Stop the line after a bad landing | `ship.py` | Post-land tier and freeze | Built-in |
| Nightly suites and the Gherkin matrix | `nightly.py`, `gherkin_nightly.py` | Recurring graphs | Built-in, with scripts for the matrix |
| Negative control | `negctl.py` | Optional gate | Built-in |
| GitHub issue labels `pipeline`, `pipe:proposed`, priority labels, blocked-by | `tickets.py`, `github.py` | GitHub tracker mapping | Built-in |
| Filing skill, ticket lint and caps | `.claude/skills/file-tickets`, `ticket lint` | Rewrite the skill for `mason task add`. Lint becomes a project script | Extension |
| Parity gallery of both clients' screenshots | `gallery.py` | A script step | Extension |
| Weekly digest, announcements and notifications | `digest.py`, `announce.py`, `notify.py` | Digest and notifications | Built-in |
| Self-update and the pinned runtime | `selfupdate.py`, `runtime` | Not needed. MASON updates itself | Dropped |
| Its own monitor | `pipeline/monitor/` | MASON's monitors | Dropped |

**Landing mode to propose:** `direct-ci`, using the same `pr-gate.yml` checks. That workflow already accepts `workflow_dispatch`, but check that its change detection works on a ref with no PR, and fix that in this repository if it doesn't.

**Owner checkpoint:** the owner signs off on the list and the landing mode. Don't continue without that.

### Step 2: Shadow

- Write `.mason/` on a branch, and land it through `starpipe` like any other change.
- Register the project. MASON claims only issues labelled `mason`, and `starpipe` keeps claiming `pipeline`. An issue never carries both.
- The owner labels a small, varied set of real tickets for MASON.

Both pipelines can land on `develop`. Each must treat a moved base as normal, and neither may force-push.

### Step 3: Compare

Report time to land, interventions, cost and escaped bugs for each system over a period the owner chooses, normally at least two weeks of shadow work.

**Owner checkpoint:** the owner decides whether to switch.

### Step 4: Switch

1. Pause claims in `starpipe` with `starpipe pause`, and let its in-flight tickets finish. If one is parked, ask the owner whether to answer it or re-label it for MASON.
2. Stop the daemon by unloading the `com.brianm998.starpiped` LaunchAgent. Keep the plist and `~/.star-pipeline/`.
3. Remove the `starpipe heavy` wrapper from suites, and point `.claude/settings.json` at MASON's hook.
4. Update `CLAUDE.md`'s pipeline section and the file-tickets skill to describe MASON.
5. Move the `pipeline` label's meaning over, so filed tickets now reach MASON.

The old pipeline stays stopped but intact for a rollback period the owner sets. **Only the owner removes `pipeline/`.** When that happens, the note in `CLAUDE.md` saying the localization rule doesn't apply to `pipeline/` goes with it.

**Exit test:** the parity report passes and the owner has approved the switch. `s27.sh` validates `.mason/`, confirms every suite sets the GPU rule and runs a dry run of a task through the graph with no LLM.

## Stages 28 to 30: Adopting the other projects

These three have no pipeline to replace, so they're adoptions rather than migrations. Each follows the same steps, and each stage ends with its own owner checkpoint.

1. **Ask the owner** which tracker to use: GitHub Issues, or the local tracker. All three repositories are public and have no open issues today.
2. **Run `mason adopt`,** confirm what it detects against the table in the stage, and read the repository's own `CLAUDE.md` or README.
3. **Be honest about tests.** These projects have few. Configure the suites that exist, and let verdicts say `untested` or `unverified` where that's the truth. Don't write a test suite as part of adoption. Each gap goes in as a proposal for the owner to approve.
4. **Leave release workflows alone.** Release pipelines aren't part of MASON, so each project's tag-driven releases keep working as they do now. If a landing could trigger one, stop and ask.
5. **Land the setup, then one real small task** the owner picks, and report what happened.

**Exit test for each:** `sNN.sh` validates `.mason/` and dry-runs the graph. A real task has landed through MASON on that repository, and the owner has confirmed the project is on.

## Stage 28: Adopt ReelVault

**Repository:** `git@github.com:brianm998/ReelVault.git`, base branch `develop`, checkout at `~/git/ReelVault`.

| Area | What's there | Starting configuration |
| --- | --- | --- |
| Rust core | `core/`, built in `ci.yml` | Lint and unit suites through cargo |
| Swift kit | `kit/`, with `kit/Tests/ReelVaultKitTests` | `swift test` suite |
| Clients | `macos/`, `ios/`, `kotlin-desktop/`, `android/`, `shared/` | Build suites. Client tests only where they exist |
| CI | `ci.yml` and `android-ci.yml`, triggered by pushes and PRs on `main` and `develop` | `direct-ci`. Add `mason/ci/**` to both triggers |
| Rules | `CLAUDE.md`, and `CLIENT_COMPARISON.md` for client differences | Prompt pack |

The repository has many `claude/*` branches from earlier sessions. Don't touch or delete them.

## Stage 29: Adopt djukebox

**Repository:** `git@github.com:brianm998/djukebox.git`, base branch `develop`, checkout at `~/git/djukebox`.

| Area | What's there | Starting configuration |
| --- | --- | --- |
| Shared code | `DJukeboxCommon/`, with tests | `swift test` suite |
| Server | `djukebox_server/`, a Vapor package with `Tests/AppTests` | `swift test` suite. Check whether these tests need a running service, and if so, make a Docker Compose environment or tell the owner |
| Client package | `DJukeboxClient/` | Build suite |
| Apps | `ios_client/` and `mac_client/` Xcode projects, with `djukebox.xcworkspace` | Build suites |
| CI | `ci.yml`, triggered by pushes and PRs on `main` and `develop` | `direct-ci`. Add `mason/ci/**` to the triggers |

## Stage 30: Adopt DiskFree

**Repository:** `git@github.com:brianm998/DiskFree.git`, base branch **`main`**, checkout at `~/git/disk_free`.

| Area | What's there | Starting configuration |
| --- | --- | --- |
| App | `DiskFree/DiskFree.xcodeproj` | `xcodebuild` build suite |
| Tests | `DiskFreeTests` and `DiskFreeUITests` | Unit suite, plus a UI suite on the `exclusive` display resource |
| CI | Only `release.yml`, triggered by `release/*` tags and by hand | `direct` landing, since no CI runs tests. Offer a test workflow as a proposal, which would allow `direct-ci` later |

Completing this stage completes M8.

## Stage 31: Version 1.0

**Goal:** a public release.

**Read:** Parts 22 and 24.

**Before you start, ask the owner:** to confirm the legal review of the App Store permission (Stage 0) is done, and for the signing identities and store accounts. Never handle these credentials yourself. The owner enters them.

**Build:**
- Signed and notarized macOS builds, Sigstore signatures and an SBOM, with an updater that verifies before switching.
- Android APKs from releases and F-Droid first, with Play optional.
- iOS build-from-source instructions, and TestFlight and App Store builds under the additional permission.
- Complete docs: install, each adapter kind, the adapter guide, writing extensions, the threat model, and the licence note on generated output.
- A security review against the threat table in Part 22, including prompt injection through issues, comments and logs. Record the findings, and fix or explicitly accept each one with the owner.

**Exit test:** `s31.sh` verifies a release artifact's signature and SBOM, then installs and updates from it. CI, conformance and every earlier exit script that can run automatically all pass on the release commit.

**Owner checkpoint:** publishing is the owner's decision. Prepare everything, send them the checklist with the results, and publish only after a clear yes.

## 5. If you get stuck

- **A design question:** ask the owner. Put one question at a time, with your recommended answer.
- **A failing exit test you can't fix:** report it with the output and what you tried. Don't start the next stage.
- **A fallback from Stage 1 isn't holding up:** stop and tell the owner, since it may change Part 6.
- **A stage is far larger than expected:** split it, keep the same exit test and tell the owner.
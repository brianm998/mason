# 0003: Readings made while building the walking skeleton (Stage 6)

Where the plan or design left room, Stage 6 chose the following. Any can be overruled.

- **The fixed graph's review step is a pass-through.** The reducer's states
  include `review`, but the Stage 6 graph is explore, develop, test, land. The
  review job succeeds immediately; a real review step arrives with Stage 7's graph.
- **The local tracker is the event log.** A task's title and body travel in the
  `TaskFiled` event (last in the payload, so older logs still decode), filed from
  `mason task add`. No service is involved. Tasks filed this way are trusted as
  the owner's.
- **Projects are local-overlay files** (`<config dir>/projects/<id>.toml`: path,
  base, remote, test command, agent binary, limits). The versioned
  `.mason/mason.toml` definition is Stage 7.
- **One event loop.** The daemon's async layer runs on a *current-thread*
  runtime, so a handler that blocks really stalls the loop and the watchdog can
  see it. A multi-threaded runtime would only stall one worker. Handlers do
  nothing but dispatch to the bounded blocking pool; jobs run on their own
  threads, bounded by admission.
- **The CLI speaks plain HTTP/JSON on loopback**, discovered through
  `daemon.json`. Stage 11 replaces this with the protobuf WebSocket the monitors use.
- **`direct` landing needs a remote**, or a base branch that is checked out
  nowhere (then it moves the local ref compare-and-swap). Landing without a
  remote on a checked-out base needs the `local-only` mode (Stage 14).
- **Test verdicts are the exit code only.** "Proof beats claims" (zero tests run
  is red) is Stage 9. Red tests loop back to developing, with the output as
  feedback, up to `max_test_rounds` (3), then the task fails.
- **Acceptance is durable, questions are not yet.** An accepted submission is
  written to disk before the agent is told; the crash harness showed the daemon
  otherwise forgets it and fails a step that had succeeded. An `ask` or a
  suspended check is still only in memory: after a restart the agent is nudged
  instead. Stage 10 makes questions durable and answerable (until then an `ask`
  leaves the task in needs attention with no way to answer it).
- **A restarted step restarts its token totals.** The session continues (same
  host, same journal), but the runner's running sums start again.
- **A provider hold waits a fixed time** (`--hold-retry-s`, 60 s by default) and
  then resumes the session, rather than parsing the reset time.
- **Admission is first-in-first-out** up to `--max-concurrent`. Ranking and
  resources are Stage 8.
- **Cancel keeps the worktree on disk** (and the branch, unless asked): only a
  landed task, or one cancelled with `--delete-branch`, is cleaned up.
- **Agents use the owner's normal Claude settings.** The first real run showed
  an agent picking up the owner's personal user-level CLAUDE.md through `HOME`.
  Stage 6 first restricted agents to project settings (`--setting-sources
  project,local`); **the owner chose to allow user settings**, so that
  restriction was removed. This means user-level instructions, and any user-level
  hooks, apply to unattended agents. Agents also tend to add a `Co-Authored-By`
  trailer, which is Claude Code's own default.

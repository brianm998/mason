# 0007: Readings made while building attention and overrides (Stage 10)

Where the plan or design left room, Stage 10 chose the following. Any can be overruled.

- **A card that does not fit is never refused; it is replaced.** The design says the
  `ask` tool "rejects anything that doesn't fit" and also that a failing card gets a
  rewrite and then a deterministic fallback so no question is lost. Both hold: the
  card as written is rejected (the agent is told exactly what was wrong, and that it
  was rewritten), and what the owner sees is a card that fits. Refusing outright would
  only make the agent loop, and a looping agent is the failure this part exists to
  prevent. Every card records its **origin** (`agent`, `rewritten`, `fallback`) and the
  problems found, and `mason task show` says so.
- **The rewrite is a project command.** `card_rewriter = "..."` in the project's
  local file names a cheap command that reads `{"card": ..., "problems": [...]}` on
  stdin and prints a card as JSON (a small model, or a script). Its output must itself
  fit or it is ignored. Without it, or if it fails or times out (30 s), the
  deterministic fallback is used. The fallback keeps the first sentence of the ask
  (cut to 200 characters), moves everything else into the evidence (trimmed to ten
  lines, noting how many were dropped), gives any one-word option a lead-in so it is a
  complete answer, adds "go ahead with your best judgment" and "stop and wait" if
  there are fewer than two, and recommends the agent's marked option or else the
  first. The full original text is in the agent's journal (`mason logs`).
- **What "fits" means.** Ask: one line, one sentence (a second sentence ending
  in `. ` and more words is refused), 200 characters at most. Why: required, 400
  characters at most. Options: 2 to 4, each a complete answer (two words or more, or
  twelve characters), 160 characters at most, exactly one marked `recommended`.
  Evidence: at most 10 lines of 300 characters. Links and a `key` are optional.
- **The card is stored as JSON in the question text.** The event log, the reducer and
  the store did not change. A question logged before this stage (bare text) is shown
  as a card with no options.
- **The hash is of the stored card text** (FNV-1a, 16 hex characters). An answer must
  carry the hash of the card the answerer was looking at; if the question was
  answered, replaced or asked again, the hash differs and the answer is refused with
  a 409. The CLI fills in the current hash when none is given, so it protects against
  a stale card only for clients that send the one they showed (every monitor will);
  `--card HASH` is there for scripts and for the exit test.
- **"Owner's identity" is a list of names the daemon is given** (`--owner NAME`,
  repeatable; the default is the user running the daemon), and the client says who
  it is (`--as NAME`, default `$USER`). This is deliberately thin: until the protocol
  stage adds authenticated clients, the loopback API trusts the name it is told, and
  what it enforces is that an answer from any *other* name (an agent, an outside
  commenter) never resumes a task. Such an attempt is recorded and counted separately,
  never as an intervention.
- **An answer is an option number, an option label, or free text** (the "Other"
  box, stored as `Other: ...`). The decision the owner made is written to the task's
  `decisions.txt` and added to the resumed agent's prompt, so it is not asked again.
- **A question about the same thing is acknowledged, not asked again.** Cards carry
  an optional `key` (a gate's name, say). If a task asks about a key it already has an
  answer for, the new question is opened and closed at once with that answer
  (`(acknowledged, not asked again) ...`), the owner is not bothered, and it is
  recorded as an acknowledgement (not an intervention). Nothing in the pipeline asks
  the owner about a gate yet (gates send work back to development); the mechanism
  is ready for the gates that will, and is exercised by an agent asking twice.
- **Pauses are runtime overrides with an owner and an expiry**, per project or global,
  in three modes: `claims` (nothing queued is claimed), `landing` (landing steps wait),
  `all` (both, and no step starts). **`all` stops work at the next step boundary:
  a running step finishes.** Stopping an agent mid-turn is a different, riskier
  override and is not part of this. A task held by a pause shows why (`paused by NAME
  (mode for scope)`) and the wait is recorded as with any other. A pause runs out by
  itself, and lifting one lets claims go at once.
- **Every override leaves a record, whichever client made it.** One piece of
  middleware on the API records each successful cancel, retry, approve, reject,
  explore-anyway, per-task override, skip, land-now, force, pause, resume and
  project override, with who and, if given, why. Answers and open-session record
  themselves. A reason is always optional.
- **Land now and force are recorded and do nothing yet.** They add a debt
  (`landing:land_now`, `landing:force`) and an override record and say plainly that
  landing control arrives in Stage 14.
- **Open session** finds the agent's session in the step host's journal and, if the
  step is idle, runs the project's `claude` binary with `--resume` in the task's
  worktree. If the step is running it refuses unless `--anyway`, since a second
  session would collide with it.
- **Alerts are computed from live conditions**, not raised by events: a question
  waiting for the owner, a failed task, an environmental wait, a refused definition,
  a pause in force, skipped checks still owed. A table keeps each alert's first
  appearance and when it cleared, so history survives; reading alerts brings the table
  up to date, and so does the daemon's background watcher.
- **The metric.** Interventions per landed task counts what a person did (overrides,
  answers, approvals, rejections, open-session). It leaves out acknowledgements by
  the pipeline and answers refused for coming from someone else. With no landed task
  there is no ratio, and the output says so rather than dividing by zero.

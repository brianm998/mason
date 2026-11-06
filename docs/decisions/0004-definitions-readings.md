# 0004: Readings made while building definitions and configuration (Stage 7)

Where the plan or design left room, Stage 7 chose the following. Any can be overruled.

- **A new crate, `mason-definition`.** Part 2's crate list has no home for the
  definition language, and `mason-core` and `mason-engine` must stay free of
  dependencies and IO (the `s02` proof). The new crate holds parsing, writing,
  validation, the configuration layers, the dry run and the clarity check. It is
  pure apart from reading the text it is handed.
- **Real TOML.** Part 5's example puts several keys on one line separated by `;`,
  which is not TOML. A definition is real TOML: one key per line, or an inline
  table. Unknown keys are errors, with a "did you mean" hint, so a typo cannot
  silently turn a setting off.
- **Events carry the resolved next step.** The reducer stays pure and does not
  read a definition. The daemon, which has the pinned definition, puts the next
  step's name and category in the event (`StepSucceeded{next}`, `SentBack{to_step}`,
  `Admitted{entry}`, ...). The reducer still enforces the category rules: a step
  may be followed by one in the same category or a later one, never an earlier
  one (going back is only by sending work back), and `Landed` only from landing.
  New fields trail the payload and are optional, so logs written before this
  stage still decode and replay with the older fixed progression
  (`stage6_*.events` fixtures prove it).
- **The pin is the definition text's hash.** A task is pinned when it is
  admitted, to the hash of the `.mason/mason.toml` it was read from on the base
  branch (or the built-in text). Definitions are stored by hash in the event
  store, so a pinned task always finds its definition. Layers 3 to 5 are *not*
  part of the pin: they are live settings.
- **The upper layers set attributes only.** The local overlay, runtime overrides
  and per-task overrides may set `model`, `effort`, `budget`, `max_rounds` and
  `command` of a step. Anything that changes the graph or the gates (`next`,
  `on_red`, `type`, `tools`, ...) is refused with a pointer to
  `.mason/mason.toml`. Runtime overrides must have an owner and an expiry (the
  store refuses one that lacks either); per-task overrides have an owner and live
  as long as the task. Both are rows beside the log, not events, because they
  are configuration, not task state.
- **Reload.** A watcher notices a changed overlay file or base-branch commit
  every 5 s, and fetches each remote every 60 s (both configurable); an explicit
  reload always fetches. A valid new definition applies to tasks admitted from
  then on; an invalid one is refused, reported (in `mason status`, `definition
  show` and `doctor`) and cleared by the next valid one, while the current
  definition keeps running. After a restart the last good one (kept in the store)
  is used if the base branch's is broken.
- **What can run now.** `agent`, `command`, `test`, `approval` and `landing`
  steps in a line with bounded loops back. `gate` (Stage 9), `ci` and `pr`
  (Stage 15), fan-out and `when` conditions are valid *language* (the validator
  checks fan-out and join structure) but are refused when a definition is
  applied, with the stage that adds them. `test` is a command step with an exit
  code verdict; suites and proof come in Stage 9.
- **The built-in definition** is the Stage 6 graph minus its pass-through review
  step: explore, develop, test, land, with `explored_entry = "develop"`.
- **Approval steps hold no slot.** A task waiting for the owner sits in the
  step's category with an "approval by the owner" wait, and does not count
  against `--max-concurrent`. `mason task approve` lets it go on; `reject` is red
  (back to `on_red` if the step has one, otherwise the task fails with the reason).
  A reviewer agent's `changes` verdict is red in the same way.
- **The clarity check** wants a named file that exists on the base branch (a bare
  name like `token.rs` matches by file name; a file to be created says `(new)`),
  an "acceptance criteria" section with items, and an "approach" section with at
  least 8 words. Uncertainty explores. The optional classifier is a project
  command (`clarity_classifier = "..."`) reading the body on stdin; only the word
  `clear` skips exploring, and only when the rules are unsure.
- **The explored flag** counts only for the owner and the owner's agents, or once
  the owner approved the task, and only when the definition has an
  `explored_entry`. Which route a task started by is recorded on it, along with
  how many times the developing agent sent it back (`request_exploration`, never
  a failure). `mason status` shows the digest's numbers now.
- **The CLI leaves quietly on a closed pipe** (`mason ... | head`) instead of
  panicking.

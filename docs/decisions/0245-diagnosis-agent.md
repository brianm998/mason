# 0245: a diagnosis agent for tasks that fail on their own code

Stage s250. Status: accepted by the owner.

## Context
A failed task waits for the owner; 50 of one week's 161 hand retries followed "still red after N rounds". `mason triage` only records suggestions and a retry carried no message to the next round.

## Decision
- When a task fails with class `code`, one read-only session of tier `light` runs in the task's worktree. It is shown the title and body, the plan, why it failed, what each round was told, the tails of the newest logs and `git diff <base>...HEAD --stat`. It cannot change files and ends with one `submit_diagnosis`: `retry` (a step and a note), `explore` (a reason), `close` (`already_done` or `duplicate` of a task, with evidence) or `ask` (a question and a recommendation, first).
- At most two sessions per task, counted durably in `diagnoses` before the session runs. A session that ends with no verdict still counts.
- For the self project (`[self]`), a `retry` is applied by itself, at most once per task per day. Every other verdict (and a second self retry within the day) is a pending diagnosis the owner applies or dismisses.
- `mason task retry --message` and a `message` on the HTTP retry give the next round a note, shown once in its prompt.
- The session takes the same `llm:<provider>` slot an agent step takes, through the broker (not `compile` or `test-suite`), waits for it like any request and releases it when it ends.
- `diagnose = false` in the project file turns the sessions off.

## Not done
The card is an alert plus two actions, not a `QuestionAsked`, because the engine takes a question only from a step and a failed task has none. `explore` applies by retrying the first exploring step with the reason as its note.

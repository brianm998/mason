# 1101: A review finding is blocking or minor

## Context

Any `changes` verdict cost a develop, test and review round, including for wording points on work the reviewer called correct.

## Decision

- `submit_review` requires `severity` on each finding. Only `blocking` sends a task back; the contract refuses `changes` without one, and `approve` with one (the two would contradict each other).
- Blocking means: it won't build; it is wrong for every input; a new or changed test passes on the base code too; an error is swallowed or a fallback hides a failure; it breaks a quoted CLAUDE.md or prompt-pack rule. Problems already on the base, what a linter or the test step catches, wording, style, naming and minor nits are not findings.
- The rule is in the tool description as well as the default prompt, because a project's prompt pack replaces the prompt.
- Minor findings are kept per task. After an approval they are given to a develop round only if one happens for another reason, and they are dropped from the pending advice when that round submits.
- Only minor findings are listed with the task; blocking ones are in the feedback history of the round they caused.

## Consequences

A reviewer session that omits `severity` gets one rejection and corrects itself.

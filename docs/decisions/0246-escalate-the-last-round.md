# 0246: use a stronger model for the last round

Stage s251. Status: accepted by the owner.

## Context
`deep` maps to Sonnet on this machine; 418 of 421 recorded develop runs were Sonnet. The model ladder moves up only for a crashed or silent agent, never after a red round, and 43 test steps in one week ended "still red after N rounds".

## Decision
- The last round a task can still pass in (the next red test round would fail it, and at least one red round has happened) runs the develop agent on `escalate_model`, in a fresh session whose prompt carries every earlier round's feedback. Earlier rounds keep their model.
- `escalate_model` is a project-file key: default `opus` for the Claude provider, none for Codex; an empty string turns it off. A step whose model is already that model is unchanged.
- The model replaced is recorded in `step_launches.moved_up_from`, and `model_from` says "escalated for the last round", so the task page and `mason task show` show it.

## Not done
The round count is the test step's red rounds since the last restart by the owner; a task that the test step does not send back to this step (review, for example) is not escalated. No cost cap beyond the existing budgets.

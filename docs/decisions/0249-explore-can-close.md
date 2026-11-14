# 0249: let exploring close a task that is already done or a duplicate

Stage s254. Status: accepted by the owner.

## Context
Explore could only call `submit_plan`, `ask` and `file_proposal`. `ClosedNoChange` existed but nothing emitted it, so an explorer that found the work already done had to invent a plan or ask.

## Decision
- `submit_close` is a terminal submission of the explore step: `kind` (`already_done`, `duplicate` with the other task's number, or `not_a_problem`) and `evidence`.
- The task is not closed at once. It asks the owner a decision card, "Close #N: <kind>: <evidence>", with "Close now" (recommended) and "Reopen and explore", and waits. The window is 24 hours for the owner's own tasks and 2 hours for tasks MASON's agents filed.
- When the window ends with no answer, the task is closed with no change (`ClosedNoChange`). "Close now" closes it at once. Any other answer reopens it: it resumes in the first exploring step and the owner's words are in what the explorer reads.
- The pending close is a row in the store (`explore_closes`), read by a loop, so it survives a restart.

## Not done
The veto window runs on the question of the task being open, so a task with a pending close shows as needing the owner like any question. Tasks filed by MASON with the owner's trust get the owner's window.

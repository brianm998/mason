# 0515: the tool-call limit covers the queue wait

Status: accepted.

The agent CLIs take one fixed tool timeout at start, so the limit cannot begin when a check gets its slot. We chose a static bound: twice the step budget (the longest the step can live, and so the longest a check can queue) plus the test timeout and 60 s. Rejected: returning "queued" at once with a follow-up call, which needs a new tool protocol. Cost: a hung call can now wait longer before the agent gives up; the step deadline still ends the step.

# 0853: the deadline extender counts from the step's launch

Status: accepted.

The host's limit counts from when the host started, so a restarted daemon does not begin the count again. The progress extender (s401) did begin it again, so a committing develop agent got no extra time after a restart and was stopped at the base budget plus credit.

Decision: the extender's clock is the time since the step's recorded launch. Only when progress time is granted changes; no limit or cap does.

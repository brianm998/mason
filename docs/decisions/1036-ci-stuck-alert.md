# 1036: alert on a CI run that stops changing

Status: accepted (s1043)

A pending run can hang while the service answers normally. The daemon now compares each poll's job-state signature (counts of queued, running, passed and failed jobs, plus running job names) with the last. Unchanged for `[ci] stuck_after` while something is unfinished raises the existing `landing` alert kind (key `ci-stuck:b<batch>`). It only observes: `timeout_s` still decides when to give up, and nothing is cancelled.

Choices: a run with no job detail (command, Jenkins, generic CI) counts as unfinished with one constant signature, so it alerts after the same time. The clear happens after `ask` returns, so a green, red, yield, timeout or error of the wait all remove it. The duration in the text is floored to steps of `stuck_after` so the alert text does not change on every poll.

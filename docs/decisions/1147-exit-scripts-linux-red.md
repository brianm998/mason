# 1147: the analyst's submission is not validated as a change

## Context

`exit-scripts (ubuntu-latest, 2)` was red twice (run 45788017032): s914, through s777 (fixed separately by 43f71f6f0), and s1035, through s22. s22 showed that the analyst's task could never be submitted: its instructions (commit nothing, leave the result file uncommitted) are what the change validator refuses. This also affected real pipelines, not only the test.

## Decision

The develop step of the task recorded as the analyst's (`is_analyst_task`, the schedule row, not the title) has no change validator. Its branch is never merged (decision 0870), and `finish_agent` still sends it back to develop when the result file is missing or unreadable. Every other task is validated as before.

## Alternatives

- Widening s22's 60 s wait: the task had already failed, so no wait would help.
- Having the analyst commit its file: it contradicts decision 0870 and leaves the instructions and the check to drift apart again.

## What was ruled out, and what a next red shows

A slow runner and a Linux-only difference: the failure reproduced on macOS. s244 and s728, which s1035 runs after s22, were not the first failure; if one goes red, s1035 now names it with its exit code and output.

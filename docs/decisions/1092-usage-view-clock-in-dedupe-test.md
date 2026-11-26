# 1092: keep s1089's fix for the flaky dedupe test

## Context

This task and s1089 fixed the same flaky test in the same way. s1089 landed first. Decision 1078 records the approach.

## Decision

Take develop's test unchanged and add no second version of the fix.

## Rejected

- Keeping this branch's version, which compared decoded usage content on a demo daemon: a demo daemon resends views at each new seq, so it does not exercise the production dedupe.

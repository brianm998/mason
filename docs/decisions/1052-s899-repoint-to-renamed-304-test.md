# 1052: s899 repoints to the renamed 304 test

## Cause

The test was renamed to `a_304_changes_nothing_and_asks_for_nothing_more` and now builds its GitHub through the `source()` helper, which calls `token();` first. s899 still looked for the old name, so it failed with no output. s1035 failed only because it reruns other scripts.

## Decision

s899 checks the contract where it now lives: the test uses `source(` before any `GitHub::new`, and `source()` calls `token();` before `GitHub::new`. It still fails on a miss.

## Not diagnosed

s22 (proposal wait timed out) and s40 (`conformance features` exit 1 at line 76) are unchanged. They need a Linux run reading the first failing line.

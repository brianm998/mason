# 1171: watchdog tests without a real timer

## Context

`CI/monitor-swift` failed once with 2 failures and passed on rerun. The log tail did not name the tests. Code reading found that fake-driven WatchdogTests started a real 250 ms timer whose tick can race the hand-driven ticks on a slow runner.

## Decision

Fake-driven and note-only tests configure the watchdog with `arm(config:log:)`, which creates no timer. No sleeps, retries or longer timeouts were added.

## Consequences

The two failures are not confirmed to be these tests until the full log of run 45883735030 is read. If other tests were the failing ones, this race is fixed but the flake remains.

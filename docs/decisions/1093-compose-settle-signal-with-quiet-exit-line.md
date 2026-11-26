# 1093: Compose settle stays signal-based and reports a quiet exit

## Context

The t1056 merges resolved `Tool.kt` to [1026](1026-compose-conformance-signal-settle.md)'s signal settle and dropped the quiet-exit line that [1057](1057-monitor-compose-settle-counts-events.md) relies on. `scripts/exit/s1068.sh` and `s1026.sh` went red on develop until s1100 changed what they check.

## Decision

1026's behaviour is wanted. The `lastViewSeq` test is already in it. The diagnostic is restored: `settle()` prints `settle: quiet exit, views at N, daemon seq M` to stderr when it returns on the agreed round trips with `lastViewSeq` below the daemon's seq. The covered exit prints nothing. Limits and compare rules are unchanged.

Rejected: restoring the `QUIET_TICKS` quiet window, which 1026 removed because a loaded runner outruns it.

## Exit tests

s1100 landed first and repaired `s1026.sh` and `s1068.sh` for the signal settle. It also refuses a `views at` check in either script, so the restored line is asserted by `scripts/exit/s1104.sh` alone, and this change edits no existing exit script.

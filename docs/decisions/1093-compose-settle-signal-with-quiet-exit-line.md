# 1093: Compose settle stays signal-based and reports a quiet exit

## Context

The t1056 merges resolved `Tool.kt` to [1026](1026-compose-conformance-signal-settle.md)'s signal settle and dropped the quiet-exit line that [1057](1057-monitor-compose-settle-counts-events.md) relies on. `scripts/exit/s1068.sh` and `s1026.sh` went red on develop.

## Decision

1026's behaviour is wanted. The `lastViewSeq` test is already in it. The diagnostic is restored: `settle()` prints `settle: quiet exit, views at N, daemon seq M` to stderr when it returns on the agreed round trips with `lastViewSeq` below the daemon's seq. The covered exit prints nothing. Limits and compare rules are unchanged.

Rejected: restoring the `QUIET_TICKS` quiet window, which 1026 removed because a loaded runner outruns it.

## Trust

`scripts/exit/s1068.sh` and `scripts/exit/s1026.sh` are protected (`scripts/exit/**`), so the owner must approve the edits. Each drops its `QUIET_TICKS` assertion, and `s1068.sh` now greps for the covered test in place of the dropped `val reached` line.

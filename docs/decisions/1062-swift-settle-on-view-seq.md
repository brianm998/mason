# 1062: Swift settle waits on view seq

Builds on [1057](1057-monitor-compose-settle-counts-events.md).

The Swift tool's `settle()` tested `lastSeq`, which events move. We match Compose's `lastViewSeq` rule rather than widening the quiet window: a wider window only hides the race and slows every checkpoint. Deadlines and compare rules are unchanged. The cause is a hypothesis until s49 step 2 is green on macOS.

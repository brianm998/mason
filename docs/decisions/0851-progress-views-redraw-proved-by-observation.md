# 0851: progress-view redraw is proved by observation, not by a render count

The three `> 0` progress-view assertions in RowRedrawTests depended on SwiftUI running observation updates inside an offscreen ImageRenderer. On the macos-26 runner the count stays 0 even with the run-loop drain of s811 (`s237: ... 0 progress views`). Decision 0785 required a replacement for any removed precondition, and 0708 forbids loosening the gate; so the guarantee moves to a deterministic MonitorCoreTests test (`testAProgressPushFiresOnlyItsTasksProgressEntry`) that uses withObservationTracking: a push for task 1 fires task 1's entry once, task 2's entry zero times, and the sidebar zero times. The RowView upper bounds, the `before > 0` rows-drawn precondition and the sidebar/pane/top-bar equality checks stay. The `s237:` print line is unchanged.

Deviation from the plan: the fixture uses tasks 1 and 2 (the rows `model()` provides) instead of 7 and 8.

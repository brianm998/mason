# 0232: Sidebar rows take data by value and are skipped when unchanged

The monitor used a core while the pipeline was busy because every `RowView` observed the whole `MonitorModel`, so any change (each sequence number, log line, clock anchor) re-evaluated every row, and each running row had its own one-second `TimelineView`.

**Decision.** Rows get a `TaskRow` plus the few values they need (`selected`, `canAct`) and closures, and are `Equatable`. High-frequency state moves to `LiveState`, observed only by the log section and the Health line. The clock offset is not published; progress lines read it on one shared tick (`SharedTick`), which stops when the app is occluded. The sidebar `ForEach` is keyed by entry id. Compose gets the same keys.

**Alternative not taken.** Moving the model to `@Observable`: a larger change touching every view, with the same effect for rows.

**Trust.** No protected path, no gate or test changed. Accessibility identifiers are unchanged.

**Not verified:** nothing was compiled or run in this sandbox; see the stage note.

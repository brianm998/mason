# 0268: the sidebar fit test follows RowView's initializer

**Decision.** `SidebarFitTests` builds `RowView` with the same arguments the sidebar passes (`row`, `selected`, `canAct`, `clock`, `go`, `approve`). The model is passed to a view only where that view reads it.

**Why.** s237 made rows take data by value; the test was stale and broke the suite's build. The width assertions are unchanged.

**Trust.** No protected file, gate, limit or assertion is touched.

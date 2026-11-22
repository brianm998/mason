# 0865: the sidebar scroll view gets one identity per project

Task #926. After a project change the sidebar's `LazyVStack` went from six entries to hundreds inside the same `ScrollView`, with a stale offset and re-estimated content heights.

Decision: `.id(model.project)` on the `ScrollView`. A project change builds a new scroll view at the top. Updates within a project keep the id, so realised rows are kept (entry ids are stable: `h:<state>`, `r:<id>`, `p:<title>`, `rule`). The `scrollTarget` path is unchanged and is covered by a test after a project change.

Not done: the `LazyVStack` is kept and was not replaced by a plain `VStack`; no measurement of a `VStack` against `LazySidebarTests` was made. The six-entry interim list is still drawn between `sidebar = nil` and the new sidebar, but in a fresh scroll view, so it cannot leave a stale document height. If the jump is seen again, the next step is a same-kind placeholder in that gap.

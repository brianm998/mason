# 0523: Live progress lives in a per-task store, not in the sidebar

Status: accepted (task #517).

Context: each StepProgress flush copied the 1.2 MB `SidebarView`, patched rows and reassigned `@Published sidebar`, redrawing every view holding the model and discarding the entries cache, about once a second.

Decision: `ProgressStore` holds an observed `TaskProgress` per task; `apply(progresses:)` writes only there. Views read live cells, falling back to the view's cells. A view from the daemon clears the entries of the tasks it carries (the daemon is the truth). The open task page has its own entry so a sidebar view cannot clear it. The protocol is unchanged.

Consequences: code that reads progress from `sidebar` rows must use `progressCells(task:)`. Rows are lazily given an entry object when drawn. The lazy sidebar and observation split follow-ups build on this.

# 0976: Blockers are one wrapping text, not an HStack of links

Stage s983. An `HStack` never wraps, so 29 blocker links shrank to one-character columns. Decision: draw the list as one `Text` with link runs (`BlockerLine`), so it wraps by whole links. The sidebar names the first three and "and N more" (opening the task page); the activity line is shortened to a count plus three. The page omits its "waiting for" line when `BlockersView` lists the same tasks. Rejected: a flow `Layout` (`MetaLayout`'s give-way order must stay untouched).

# 0527: Lazy sidebar list and a single meta-line layout

Status: accepted (task #518)

Context: a non-lazy `VStack` measured all rows (517 with Landed and Canceled open) on every layout pass, and each row's `ViewThatFits` measured up to four variants.

Decision: `LazyVStack` keyed by `SidebarEntry.id`; scroll targets use the entry id (`h:<state>`) and the header's own `.id` is removed so the ForEach identity is the one scrolled to. The meta line is one `Layout` that chooses a variant from the ideal widths, caching them once per row. Droppable pieces are clipped to zero width rather than removed, since a `Layout` cannot hide a subview.

Consequences: row cost scales with the viewport. Dropped pieces stay in the view tree and possibly in accessibility; none carries an identifier. Not verified on a running Swift toolchain.

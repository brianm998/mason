# 0977: The Failed tile opens a Failed page

Stage s984. Queued, Developing, Testing, Review, Needs you and Proposed open a stage page on a
double click; Failed did nothing with two or more tasks.

Decision: `failed` joins `Page.sectionNames` in both clients, so the generic stage page, the
sidebar `>` button and `mason://section/failed` all work. No home section is added. With no failed
rows a double click does nothing. A capped page shows "N of M". Retry/Cancel stay in the sidebar.

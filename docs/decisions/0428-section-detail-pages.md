# 0428: Section detail pages reuse the sidebar's rows and the existing Landing page

Decision: every home section gets a page at `mason://section/<name>`, and each page draws what the client already has, the sidebar's project-scoped rows in the sidebar's order, rather than a new daemon view. `landing` stays the existing Landing page (`mason://section/landing` parses to it) instead of a second landing page. A double click on a count tile opens the lone task when the stage holds exactly one row, else the stage's page; a single click still only reveals the sidebar section.

Why: the sidebar already carries the sorting (not waiting first, then age), the project filter and the skip actions, so the pages cannot disagree with it, and no protocol change is needed. One landing page avoids two places to maintain.

Rejected: a new view kind per section (more daemon work for the same rows); keeping the double click on a many-task stage as reveal only (the task asks for the page).

Not verified: see `docs/stages/s435.md`.

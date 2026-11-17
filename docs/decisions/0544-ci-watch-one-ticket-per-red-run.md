# 0544: One ticket per red run

Status: accepted (s551).

Context: the base-branch CI watch filed the same red run 15 times (star, 19 tasks from two runs). The long-red rule refiled every threshold whether or not a newer run existed, and the duplicate check saw only open tasks.

Decision: a job keeps the newest run a ticket covers (`covered`). The long-red rule files again only for a newer run, and, if that ticket landed a change, one created at or after the landing. Tasks that landed or closed in the last 7 days count: one whose body cites the run is recorded as the job's ticket and nothing is filed. The rule stays pure in core; the daemon passes the clock and the recent tasks. Held-back jobs are shown in `mason status` and as monitor banners. Agent proposals also refuse a near-duplicate of a task that landed or closed in the last 7 days.

Consequences: a gate freeze is unchanged and holds while a refile is held. A newer red run after a landing is still filed (a regression). A job whose run no one fixes stays red and visible, but costs no more tasks.

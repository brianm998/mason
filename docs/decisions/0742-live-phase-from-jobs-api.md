# 0742: show a GitHub run's phase from the jobs API first

Decision: while a run is queued or running, the daemon reads the run's jobs with a conditional GET and shows the current step and, for a sharded run, shards done. It never shows a percentage for an uncounted run. Live test counts (ticket part 2) need the workflow to publish them and wait for the owner's approval.

Why: it needs no workflow change and no new token scope, and answers the owner's real question (slow or hung). The jobs ETag keeps it inside the rate limit.

Not decided here: the commit-status channel for counts (`statuses: write`).

# 0103: the step deadline excludes time queued for a check

Decision: queue time in `run_check` is added to the session deadline via a shared `DeadlineExtra`, instead of raising the limit globally. Test run time itself still counts. The takeover strand is a different fault and is left for its own ticket.

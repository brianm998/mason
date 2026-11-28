# 1295: a dispatch lost to an outage keeps its ref

Decision: when a dispatch fails with an outage, the run ref and a `cancel_pending` record are kept for the reaper. When the token is refused, both are deleted at once.

Why: GitHub may have taken a dispatch whose answer was lost, and deleting the ref and record would leave a run nothing else would find (s1256, decision 1246). A refused token cannot have started a run. The older test that expected no ref in both cases was changed to match; the code was not.

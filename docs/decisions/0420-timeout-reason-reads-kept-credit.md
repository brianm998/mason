# 0420: a timeout reason reads the kept queue-wait credit

Status: accepted

The in-memory wait total is lost when the daemon restarts, so a step that timed out after one lost its "mostly waiting" wording and counted as a code failure. The credit file from s417 survives, so `timeout_reason` uses the larger of the two. See s427.

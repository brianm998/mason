# 1084: conformance capture diagnostics

s40 went red once on CI because the owner and stranger captures held different card checkpoints, and nothing recorded which. The assertion now prints the differing keys, and `expected` prints how each checkpoint settled.

What `settle` waits for is unchanged. [1025](1025-demo-views-resend-settle.md) removed the quiet fallback for demo daemons, which was the candidate cause, and nothing yet shows another. The settle note stays so that a log shows how each capture ended, including a live daemon's agreed round-trip exit.

The settle result goes in the log line only, not in the checkpoint JSON, so `conformance check` and its consumers see the same format.

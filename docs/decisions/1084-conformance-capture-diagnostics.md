# 1084: conformance capture diagnostics

s40 went red once on CI because the owner and stranger captures held different card checkpoints, and nothing recorded which. The assertion now prints the differing keys, and `expected` prints how each checkpoint settled.

`settle` is left as it is until a red run shows the cause. The quiet fallback is only a candidate, and the daemon sends no view whose content did not change, so requiring the target or lengthening the window on a guess could make captures fail or slow every run without fixing this one.

The settle result goes in the log line only, not in the checkpoint JSON, so `conformance check` and its consumers see the same format.

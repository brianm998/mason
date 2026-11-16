# 0457: the CPU-gate finding counts time held under a CPU cause

Status: accepted (s464)

Decision: `overloaded:cpu-gate` judges the largest time one task has spent under WaitCause::Cpu or
WaitCause::Settling in its current wait, accumulated by the broker each pass (each pass adds at
most 10 s). It does not match on reason text and does not use the wait's whole age.

Why: text matching caught the short settle reason, and the whole age counted queue time.

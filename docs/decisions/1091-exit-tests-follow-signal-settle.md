# 1091: stale exit tests follow the signal settle and base-only suite sizing

## Context

Three exit scripts asserted on behaviour that later changes replaced on purpose: decision 1026 (Compose settle is a signal, not a quiet window) and #959 (only a run at the base itself sizes a suite; the store test `suite_size_is_not_overwritten_by_task_greens` pins it).

## Decision

Change the tests, not the code.

- s1068 and s1026 check the current `Tool.kt` markers: the view-seq predicate, the agreeing round trips, and the 60 s failure naming what was awaited. The Rust capturer's quiet-exit line is still checked, because it still exists.
- s70 still requires the passed count to rise to 40 and the final total to be 40, with source "minimum" or "expected". It no longer requires every record to show a total of 40: a minimum total follows the count.

Rejected: sizing suites from task greens again, which would undo #959 and contradict its store test.

## Trust

`scripts/exit/**` is protected; editing s70, s1068 and s1026 needs the owner's approval.

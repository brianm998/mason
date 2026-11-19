# 0636: Landing fast-tier tests batch by count, not by timer

Decision: tests that need two tasks in one batch set `max_size = 2` and a long window, so the batch forms on the second admission. Per-task setup such as skip debts runs before admission via `W::task_with`.

Why: a short window races with slow test setup on a busy machine. No timing assertion is added.

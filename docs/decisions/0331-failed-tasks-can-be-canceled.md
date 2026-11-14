# 0331: a failed task can be canceled

**Decision.** Canceled is a legal event from Failed. It keeps the branch by default, so the task can be revived.

**Why.** DESIGN.md lets cancel work from any non-terminal state, and failed is not terminal. The reducer's narrower rule left obsolete failed tasks (star #255, #310, mason #306 on 2026-11-14) with no way to close them except retrying work that had nothing left to do.

**Trust.** `scripts/exit/s337.sh` is in `PROTECTED`; `crates/engine` is not. The owner approved this change on 2026-11-14 ("Yes, fix it here"). Nothing is weakened: no gate or verdict changes, and a canceled task lands nothing.

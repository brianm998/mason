# 0547: stuck:wait follows the slot's progress

Status: accepted (s554).

Context: every task waiting past `wait_m` raised `stuck:wait:<id>`, so 23 compile waiters behind holders turning over every ~20 min gave 17 warns from one shared queue. Decision 0342 (aging respects progress) orders the queue; 0425 and 0458 made waits and holders visible.

Decision: a wait with holders raises `stuck:wait` only when no holder was granted within `wait_m`, or the wait exceeds max(`wait_m`, queue depth x longest holder run). A progressing wait older than `aged_wait_h` (default 6) gives one `stuck:aged-wait:<id>` at the new severity `note`, so starvation stays visible. Notes appear in the check record but `fresh()` drops them, so they never start a health session. Waits without holders (CPU gate, settle, disk) and `overloaded:slots` are unchanged.

Trust: the daemon publishes holder grant times and queue depth beside the explanations in `crates/daemon/src/sched.rs`, which is protected and needs owner approval.

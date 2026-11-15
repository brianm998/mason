# 0422: a batch whose testing was skipped is rebuilt, not re-tested, when the base moves

**Decision.** When the base moves while a batch verifies, and every task in the batch has its test step skipped by the owner (an open `step:<test>` debt), the batch is rebuilt on the new base and lands if the build passes. It is not re-tested.

**Why.** A skip says the owner chose to land without testing and to pay for it later, in one run. The re-test after a moved base contradicted that, and it was the slowest part of landing: a full workspace run that held the one-at-a-time queue for half an hour every time anyone pushed to the base (s429).

**Trust.** `crates/daemon/src/landing.rs`, `crates/landing/**` and `scripts/exit/s429.sh` are in `PROTECTED`. The owner approved this on 2026-11-15 ("Yes, prepare them"). The rebuild still has to pass, and the debts stay open until the full run pays them.

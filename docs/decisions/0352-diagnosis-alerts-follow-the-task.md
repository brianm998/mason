# 0352: a diagnosis alert lasts only while its failure does

**Decision.** A pending diagnosis raises its "needs you" alert only while its task is in `failed` and entered that state before the diagnosis was written. The diagnosis row is not changed: it stays pending, so the owner can still apply or dismiss it.

**Why.** A diagnosis suggests what to do about one failure. Once the task has moved on (retried by the owner or by another route, landed, canceled, or failed afresh), the suggestion no longer applies. Speaking it again is a false alarm that teaches the owner to ignore alerts (2026-11-14: #197 and a dozen others). Leaving the row alone keeps the record of what was diagnosed and decided.

**Trust.** `crates/daemon/src/diagnose.rs` is not in `PROTECTED`; `scripts/exit/s359.sh` is. The owner asked for this on 2026-11-14 ("Either there is a problem with that ticket or the monitor needs an update"). No gate or check changes.

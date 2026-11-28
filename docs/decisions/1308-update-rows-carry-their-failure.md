# 1308: one `update_failed` row per check, each with its failure

**Decision.** The health check keeps writing one `update_failed` row for every check at which the
update is failed, and each row now carries the target, phase and error as JSON.

**Why.** `gather` counts these rows against `limits.update_failures`. Writing a row only when the
target or error changed would leave one stuck failure at a count of one, so `update_blocked` and
the severe finding would never trip. Keeping the rows leaves that gate as it was.

**Not chosen.** A row per distinct failure with a repeat count: it needs rows to be rewritten, and
the 24 h window over the rows would lose a failure that started earlier.

**Not chosen.** Storing the failing phase in `update::Status`: `update.rs` is a protected file and
the log already shows how far the update got.

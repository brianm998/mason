# 0500: fix the duplicate events in the matrix test, keep the count

**Decision.** The duplicate `Deferred` and `Undeferred` entries are removed from `events()` in `crates/engine/tests/matrix.rs`. The `checked == 12 * 26` assertion stays as it is, and a new check fails if an event name is listed twice.

**Why.** The count was right and the list was wrong. Loosening the count would have hidden the duplicates.

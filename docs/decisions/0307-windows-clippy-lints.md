# 0307: fix Windows-only clippy lints in code, not with allow attributes

**Decision.** `measure_free_memory_mb` delegates to two cfg-gated functions instead of `return` in a cfg block; the portable test ends the first controller in a block instead of `drop` on a non-`Drop` value; `fake_host` in hostarchive.rs is gated `#[cfg(unix)]` like its only callers.

**Why.** These lints exist only on Windows builds, so no local step sees them. Behaviour is unchanged on every platform.

**Trust.** No protected path is touched. Not verified: a Windows clippy run (see submission).

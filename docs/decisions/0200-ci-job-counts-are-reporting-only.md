# 0200: CI job counts are reporting only, and an answered poll is a heartbeat

**Decision.** Per-workflow job counts travel beside the verdict (`status_detail`, `CheckCounts.jobs`), never inside `CiState`, so green/red decisions and the failed-job path read by #162 and #184 are untouched. A poll that gets an answer while CI is pending calls `Progress::heartbeat`, which moves `updated_at_ms` and raises `stale_after_ms` to at least three polls; no figure is invented. "No output" now means the host stopped answering.

**Trust.** Edits `crates/daemon/src/landing.rs` (protected): it adds reporting calls and one heartbeat in `ask`; no verdict, gate or ladder step changes.

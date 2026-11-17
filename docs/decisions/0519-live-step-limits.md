# 0519: running steps read their limits from the live registry

**Decision.** Limits that are safe to change mid-step (`test_timeout_s`, `test_reruns`, `step_timeout_s`, `tool_timeout_s`) are read through `Ctx::live()` each time they are used. What is fixed at start (path, base, remote, `[self]`) is read from the step's own copy, and `reload_registration` still refuses edits to those.

**Why.** On 2026-11-16 raising `test_timeout_s` from 3000 to 5400 was applied by the reload, but eight running develop steps kept 3000 s and kept losing full runs until masond was restarted.

**Trust.** `jobs.rs` and `daemon.rs` are not in `PROTECTED`; no gate, budget or verdict logic changed.

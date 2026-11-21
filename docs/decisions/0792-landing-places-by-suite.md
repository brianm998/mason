# 0792: landing checks for a named suite follow the suite's placement

Date: 2026-11-21. Decision: the owner asked for this fix in chat (#815).

The build floor, the base floor and the fast tier ran with `global: true`, which ignores the
suite's `runs_on`, `remote = false` and device rules. They now pass `place: Some((None, suite))`
so `place_of` decides, with no task owning the batch. `global: true` remains only for checks with
no suite. This touches `crates/daemon/src/landing.rs` (verdict logic, protected); gates and
commands are unchanged, only where the builds run.

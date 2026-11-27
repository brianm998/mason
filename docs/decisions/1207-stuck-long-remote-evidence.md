# 1207: stuck:long names the task's open remote runs

Decision:
- A `stuck:long` finding for a step with open remote runs (`remote_runs::open_of`) gets one evidence line per run: kind, `runs_on`, waiting flag, age of the last poll. A step without open runs gets none.
- Diagnostic only. `long_factor`, `long_min_ms`, `long_landing_min_ms`, `checks_max`, `wait_ms` and `run_check` slot acquisition are unchanged.

Reason: #1046, #1110 and #1185 crossed the 815 min limit within ~40 min of each other, silent 0 min, in their first check round. A broker slot wait above `wait_ms` is reported as `stuck:wait` and skips `stuck:long` (core/src/health.rs), so these were not a visible local slot wait. Remote waits and node runs are invisible to the judge (`remote_runs.rs` quiet adjustment excludes waiting and node runs), so the finding could not say whether the check was waiting or running.

Follow-up: change slot handling only if the next finding or the operator checklist (docs/stages/s1217.md) shows a local queue; propose a threshold change separately if the tests are shown to be genuinely running.

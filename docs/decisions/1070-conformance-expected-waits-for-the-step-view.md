# 1070: the expected capturer waits for the step's view

## Context
In run 45361042086 every expected checkpoint k>=2 equalled the state after step k-1, while the Swift capture matched the script. `settle()` in crates/conformance/src/main.rs left after 1.5 s without a new view even though the step's view had not arrived.

## Decision
Leave on reaching the target seq, or after a 5 s quiet window (the daemon sends no view for steps that change nothing visible), or fail at the 60 s hard limit. The rule is the pure function `should_leave`. The fallback prints `settle: quiet exit` on stderr.

## Alternative
Only raising the quiet window: still a race, with no way to tell a lag from a quiet step. The target stays primary and the window only a bounded fallback.

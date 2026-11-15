# 0348: "timed out" means the command ran past its limit

**Decision.** A deadline timer that fires is not enough to call a run timed out. The host's own records must also show the command was ended by a signal or ended after its limit (`registry::ran_past`). With no records, it counts as timed out, as before.

**Why.** The limit is counted from the host's start, so that a restarted daemon does not give a run a fresh limit. But the same rule fires the timer at once for a run that finished long ago, and that failed star #197 twice on a suite that had passed in 88 s (2026-11-14). The host's exit record already says when the command ended, so the verdict can follow what happened, not when the daemon looked.

**Trust.** `crates/daemon/src/suites.rs` and `scripts/exit/s355.sh` are in `PROTECTED`. The owner asked for #197 to be looked into on 2026-11-14 ("Look a what's wrong with #197"). Nothing is weakened: a run that did exceed its limit, or was stopped by its deadline, is still red.

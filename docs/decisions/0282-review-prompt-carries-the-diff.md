# 0282: a review's prompt carries the diff

**Decision.** MASON writes the change under review into the review prompt (stat and patch, at most 60,000 bytes of patch) instead of giving the review step a shell.

**Why.** The reviewer must see the change to judge it, and a read-only step should stay unable to run commands. Without the diff, reviews either refused (star #255 failed twice) or approved blind (#305).

**Trust.** `jobs.rs` is not in `PROTECTED`; `scripts/exit/s287.sh` (exit tests) is. The owner approved this change on 2026-11-14 ("Yes, all three"). Nothing is weakened: the review step's tools are unchanged.

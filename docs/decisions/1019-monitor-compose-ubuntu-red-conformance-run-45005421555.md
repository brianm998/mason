# 1019: monitor-compose (ubuntu) red in run 45005421555 is a conformance difference

## Context

Run 45005421555 (commit 38a226d26, develop) failed in `scripts/exit/s13.sh` at the `conformance check` of step 2 (line 73): "conformance failed: 21 difference(s)". It is not a `FAILED:` testcase (the Gradle step passed) and not a timeout. The 21 differences are 17 at checkpoint 12 (rows developing 5-10, testing 1/11/12/13/16, landing 14/15, proposed 4, needs_attention 2, card:2, approval:4), 2 at checkpoint 10 and 2 at checkpoint 8 (card:2, approval:4). All are items Compose showed that the daemon's views did not say were visible; at checkpoint 12 expected 18 visible items, Compose 61.

Earlier records (0620 to 0905, see 0707 and 0690) could not classify this red because their logs were cut. This one can.

## Decision

Add evidence only: s13.sh prints the per-checkpoint id diff when the check fails, and Compose's `step()` prints a stderr line when it leaves its quiet window with `lastViewSeq < seq`. No assertion, deadline, compare rule or expected value changes. Hypothesis, unconfirmed: on a slow runner `step()` dumps before the removal views arrive (Rust's `settle()` waits for all three view kinds). If the next red shows the quiet-exit line while macOS passes, make `step()` wait for all view kinds at seq. Related: s912, s714, s697.

## Trust

No protected path is touched.

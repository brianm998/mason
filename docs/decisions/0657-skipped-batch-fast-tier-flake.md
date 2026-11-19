# 0657: skipped-batch fast-tier flake: no guess, keep the evidence path

Status: accepted (s664, task #650)

Context: `a_skipped_batch_whose_merge_breaks_a_unit_test_is_parked_naming_that_test` was quarantined after flaking in 4 tasks. s643 already fixed batch formation by count.

Decision: the cause is unconfirmed. 20 runs here all passed, so no fix is guessed. Ruled out: the 900 s wait floor, git races (retried by s642), the idle debt pass (landed tasks only), `settle_debts` (landed tasks only). Open: (a) floor not stopped by skip debts; (b) fast-tier output unparsed (`fast_failures`, `hostedcheck::collect` reading before the log is written).

Consequences: the assertion `why.contains("matrix_events")` and the wait floor stay as they are; no timing assertion is added. The test prints `{why}` on failure, which classifies the next flake as (a) or (b). `scripts/exit/s664.sh` runs it 10 times.

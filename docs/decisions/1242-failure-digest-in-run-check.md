# 1242: red run_check results lead with failing names and panics

Decision: build the digest at the consumer (`jobs.rs`) from the whole log, instead of widening the 40-line tail in `hostedcheck.rs`. Why: the tail is shared with other paths and a longer tail still misses early failures under `--no-fail-fast`. A scoped suite that ran 0 tests is reported NOT GREEN; this changes verdict text for scoped runs only and needs owner approval. No protected file was changed.

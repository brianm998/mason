# 0703: Check the owner's skips before taking a suite lease

A batch whose members all skipped a suite does not take that suite's resources. The skip was already the outcome and already owed as a debt; the order changed, no verdict did. `landing.rs` is protected (landing rules), so this waits for owner approval. `run_over` keeps its check for a skip pressed while the suite runs.

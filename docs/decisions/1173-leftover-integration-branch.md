# 1173: a finished batch's integration branch is retried, not swept once

Decision: every coordinator pass deletes `mason/int/<project>/b<id>` for batches whose row is finished; running batches are never touched.
Why: a brief git failure in the one-shot cleanup must not decide an outcome (decision 0105).

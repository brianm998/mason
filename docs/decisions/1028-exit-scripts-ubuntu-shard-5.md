# 1028: exit scripts ubuntu shard 5

## Decision

The failure of s22, s244 and s728 on ubuntu shard 5/6 is not fixed by guesswork. The log that names the failing assertions was unreadable from the pipeline, so only weights entries and a check script are added; the command that fetches the log is in docs/stages/s1035.md. No assertion is removed or weakened, no skip or `|| true` is added, and no timing assertion is added. A fix follows once the cause is recorded.

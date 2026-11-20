# 0713: no duplicate s426 run in s640; s664 builds once

Decision: s640 no longer runs s426, since the shard plan runs it on its own. s664 compiles the test once and runs the binary ten times. The stale `s640 600` weight is removed; other weights are refreshed only by `scripts/refresh-exit-weights.sh` from logs.

The ten-run guard of decision 0657 is kept: the same test still runs ten times, `--exact`.

Consequences: shard 6 saves about 13 minutes. Shard membership moves when weights change, as 0661 describes; every script still lands in exactly one shard (checked by s720).

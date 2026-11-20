# 0691: CI watch dedupe on the job and on what failed

Status: accepted (the watch's filing rules; not a gate, guard or trust file)

**Decision.** A recent task covers a red job on a run only if its body cites the run and its title or body names the job (or, for a numbered shard, the shard group). Failures are read from the log (exit scripts, cargo tests) and kept on the job; shards of one matrix job are filed once under `workflow/name (os, *)`; a red whose failing script or test an open task or issue already names is attached instead of filed; a red whose failure a landed task within the lookback named is filed with that task named in the body.

**Why.** #640 (`CI/lint`) hid a real `CI/test (ubuntu-latest)` red, and one missing SPDX line was filed per shard and again after every shard reshuffle (s486 in #610, #625, #646), each time without the history.

**Kept.** The freeze, the gate, agreement and the long-red thresholds are unchanged.

**Cost.** One more log read per newly failed job. A failure no log line names falls back to the job key as before.

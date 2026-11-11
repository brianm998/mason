# 0120: The CPU gate fails closed, and reads the CPU without a process

**Decision.** With no fresh primary CPU reading the gate uses the load average over the logical CPU count, and if that fails too it treats the machine as 100% busy. It never opens because it is blind. On macOS the primary reading is a `host_statistics` syscall, not `top`.

**Why.** Holding a compile that could have started costs seconds. Admitting 12 at once cost the pipeline about an hour (2026-11-11, load 86-97 on 36 threads). The old gate failed open in exactly the case it exists for, and an open gate slowed `top` further.

**Trust.** This changes when compiles are admitted, in the strict direction only: a compile that would have started on an unknown reading now waits for the fallback. `FileCpu`/`MASON_CPU_FILE` still take precedence; no limit, guard or protected file was weakened or edited. `unsafe` is used in two small blocks with local allows (the workspace lint is still `deny`).

**Cost.** On a machine whose load average is high for reasons unrelated to CPU (many blocked processes), the fallback can hold compiles that the real CPU reading would have allowed, but only while the primary is missing.

**Not verified:** behaviour under real load; see the stage note s133.

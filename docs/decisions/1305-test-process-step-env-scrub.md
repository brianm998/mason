# 1305: the test process drops the step's variables before an in-process daemon starts

**Decision.** `Scratch::start_with` removes `STEP_ENV` from the test process's own environment.
The same list is removed from a spawned `masond`.

**Why.** The daemon runs inside the test process, so its hosts inherit that environment. There
is no child command to clean, and a check that reads `MASON_STEP` acts as a test step.

**Once per process.** The tests of a binary run in parallel, and no test sets one of these
names for its in-process daemon to read.

**Not chosen.** Having the product code clear `MASON_STEP` for a check: a daemon is not
normally started inside a step, and the leak is the harness's. Relaxing the two assertions
would have stopped them testing what a check is told.

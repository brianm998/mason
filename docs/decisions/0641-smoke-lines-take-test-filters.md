# 0641: smoke lines take a test filter

A smoke line may end in `-- FILTER...`. Cargo applies a filter to every target of one command, so a filtered line is a command of its own rather than merged into the package group. Its build drops the filter (so no test runs under `--no-run`); the run keeps it. A filtered line is skipped when the change already selects its target whole. This lets the smoke set name a module of the 543-test daemon `it` executable (decision 0539's intent: a few fast tests) instead of running all of it.

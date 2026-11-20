# 0696: narrow a directory test target to the modules a change affects

Status: accepted (`affected.rs`, not a protected file)

**Decision.** For a `tests/<dir>/main.rs` target, change-based selection asks `references` per `mod <name>;` file and runs `--test <dir> -- name::` for those modules, instead of the whole target. A changed module selects itself; `main.rs`, shared files and `#[path]` files select the whole target, as do more than half the modules.

**Heuristic.** Every module can reach any daemon code through the `masond` child, so a module that does not name the change can still exercise it. Only a module's own text counts (the harness included by `#[path]` is common to all modules and does not select them). Misses are caught by the landing batch, which runs `it` whole. A coverage-based pick refines this later and keeps this scan as its fallback.

**Cost.** Develop checks and test steps can miss a regression that only a module not naming the change would catch; it is found at the landing batch.

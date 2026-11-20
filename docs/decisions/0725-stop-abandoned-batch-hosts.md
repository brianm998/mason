# 0725: stop the step hosts of abandoned batches

A batch abandoned by a restart has its result thrown away, so its hosts are stopped at once (coordinator first pass) and, as a backstop, by the sweep's orphan pass. A batch is "running" for the sweep when its store row is unfinished; after the first coordinator pass that is exactly this daemon's batches. `landing.rs` is protected; the change adds a host stop only and changes no verdict.

# 0167: MASON owns a per-task cache directory

Decision: `MASON_TASK_CACHE` is `<data dir>/c/<task>`, separate from the project's `task_cache` template (s78), which stays for owners who name their own path. The new directory is removed on cancel as well as landing, because it is cheap to rebuild and a revived task simply recreates it; the s78 directory keeps its longer retention.

Why a new directory rather than reusing `task_cache`: MASON builds the path itself under its data directory, so no path-safety rules are needed beyond refusing symlinks, and every project has one without configuration.

Shared dependencies are left to sccache (documented, optional); the daemon has no dependency on it.

Not verified: behaviour with a real cargo build and sccache.

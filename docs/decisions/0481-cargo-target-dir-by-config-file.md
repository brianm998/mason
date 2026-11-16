# 0481: point the agent's cargo at the task directory with a config file, not an environment variable

Status: accepted

The agent's cargo built a second copy of the workspace in `<worktree>/target`. `CARGO_TARGET_DIR` would fix that but sccache hashes `CARGO_*` variables, so a per-task value made every dependency miss the shared cache. MASON instead writes an untracked `.cargo/config.toml` with `[build] target-dir` in each task worktree and lists it in `info/exclude`. A tracked config is never overwritten. The project file's `--target-dir` flag is unchanged. No protected file is touched.

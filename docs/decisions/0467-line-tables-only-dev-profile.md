# 0467: dev and test builds carry line tables only

Status: accepted

Every task builds the workspace many times. Full debug info (`debug = 2`) costs target dir space and linker time and buys nothing the pipeline uses. `[profile.dev]` is now `line-tables-only` (panics and backtraces keep file and line) and dependencies get `debug = false`. Developers who want full info set `CARGO_PROFILE_DEV_DEBUG=2`. The release profile is unchanged. Measurements are in docs/stages/s474.md.

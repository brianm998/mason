# 0472: Reap a test world's step hosts on drop, sweep orphans at start

Status: accepted (s479)

Step hosts are built to survive their daemon, so removing a test's temp dir left them running. We reap on `Scratch` drop (verified registry reap, which runs on panic) and, as a safety net, sweep processes whose `--dir`/`--data-dir` is under a `/tmp/ms*` dir that no longer exists. Matching is by that path shape only, never by process name. Only `crates/daemon/tests/support/` changed; fixtures are untouched.

# 0765: gate the command shims to unix, do not port them

Decision: the shims are POSIX sh wrappers, so `write` is `#[cfg(unix)]` and a non-unix stub returns None; the prompt line is gated with it (`cfg!(unix)` in `remote_shims`). Rejected: porting to .cmd wrappers (nothing needs them) and skipping the tests (the test module is simply unix-only).

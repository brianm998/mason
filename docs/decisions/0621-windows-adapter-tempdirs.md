# 0621: Windows adapter test scratch directories

`tempfile::tempdir_in("/tmp")` resolves to `D:\tmp` on the Windows runner, which does not exist.
The harnesses now call `common::short_tempdir()`: `/tmp` on Unix (short paths for AF_UNIX sockets),
`tempfile::tempdir()` on Windows. `sandbox.rs` and `real_claude.rs` are unchanged: they never run on Windows.

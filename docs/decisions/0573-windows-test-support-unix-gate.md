# 0573: Gate the test orphan sweep to unix

Status: accepted

Context: `nix` is a unix-only dependency; the test support module used it unconditionally, breaking the Windows build.

Decision: Put the sweep in a `cfg(unix)` module and provide a no-op on other platforms.

Trust: No protected paths touched.

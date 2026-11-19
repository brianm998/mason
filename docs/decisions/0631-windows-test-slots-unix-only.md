# 0631: Test world slots are Unix-only

Decision: gate the test-only flock slot limit behind `cfg(unix)` with a no-op on Windows, rather than add Windows file locking.

Why: Windows CI only builds the daemon tests and never runs them; the lock only limits parallel worlds.

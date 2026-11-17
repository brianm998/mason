# 0488: Compose test JVM uses its own skiko directory

Loading PNGs in tests needs Skiko's native library, which it extracts into a shared
directory. Stale or concurrent extractions made the load throw UnsatisfiedLinkError.
The test task now sets `skiko.data.path` and `java.io.tmpdir` to a directory under
`build/`. Mascot.bitmap's catch is unchanged and the assertions are untouched.

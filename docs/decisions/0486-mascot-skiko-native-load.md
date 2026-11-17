# 0486: Fix the Skiko native load in the build, not the loader

`UnsatisfiedLinkError` is an Error, and `Mascot.bitmap()` catches only `Exception`. Catching
`Throwable` would hide a broken environment and make the art silently vanish, so the fix is in
`clients/compose/app/build.gradle.kts`: the test task sets `skiko.data.path` to `build/skiko`,
so the native library is unpacked somewhere writable and fresh instead of `~/.skiko`.

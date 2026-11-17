# 0487: the Compose test JVM allows native access for Skia

Status: accepted

MascotTest and TopBarTest failed with `UnsatisfiedLinkError` because the test JVM could not load Skia's native library. The test task now passes `--enable-native-access=ALL-UNNAMED`, matching the desktop run. We did not catch `Throwable` in `Mascot.bitmap()` or loosen any assertion: that would hide the fault.

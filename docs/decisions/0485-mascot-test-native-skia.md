# 0485: Load Skia from the build directory in the Compose tests

Status: accepted

The image tests failed with `UnsatisfiedLinkError` because Skia's native library did not load from the
default unpack location. We point `skiko.data.path` and `java.io.tmpdir` of the test JVM at the build
directory. We do not catch `Throwable` in `Mascot.kt` and do not weaken any assertion: a test that
cannot load the art must fail.

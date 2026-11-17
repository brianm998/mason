# 0489: Test JVM loads Skiko explicitly

Cause: `Image.makeFromEncoded` threw UnsatisfiedLinkError (an Error, which Mascot.bitmap's
`catch (Exception)` does not catch) because the Skia native library had not been loaded in the
Gradle test JVM.

Fix: a JUnit `LauncherSessionListener` in the app's test sources calls `Library.staticLoad()`
before any test runs (with `junit-platform-launcher` as a test dependency). We do not catch
Throwable in Mascot and do not weaken any test.

# Device sample app

A tiny iOS app and a tiny Android app, each with a unit test, for trying MASON's
device test environments (Stage 25) on a real simulator, a real emulator and a
real phone. It is a sample, not an existing repository, and **it was not built or
run when it was written**: the machine that wrote it had no Xcode, Android SDK or
device attached. Expect to fix a version number or two.

```
ios/       Swift package: greeting(for:), a SwiftUI view, XCTest tests
android/   Gradle app: greeting(), an activity, a JUnit test and an instrumented test
.mason/    the definition: four suites, four environments
```

## Try it by hand first

```
cd ios && swift test                       # runs on the Mac, no device
cd android && ./gradlew testDebugUnitTest  # runs on the JVM, no device
```

## Try it with MASON

1. Copy this directory somewhere new and make it a git repository with a remote:
   `cp -R docs/examples/device-sample ~/mason-device-sample && cd ~/mason-device-sample && git init -b main && git add -A && git commit -m sample`.
2. Register it as a project (see `mason init`) and start `masond` with
   `[targets] provider = "system"` (the default) in `daemon.toml`.
3. `mason targets list` should show what MASON can see: simulators from
   `xcrun simctl`, emulators and phones from `adb devices -l`, iPhones from
   `xcrun devicectl`. A simulator may be shut down; MASON boots it for a suite and
   shuts it down after only if it booted it.
4. File a task that touches `ios/` and one that touches `android/`. Watch
   `mason targets list` show each lease and `MASON_DEVICE` reach the suite.
5. Unplug the phone, or let its certificate expire, and file another task: it
   should wait with a sentence starting "environment problem, not the task's
   fault", and an `environment` alert should appear, not a failure.

`scripts/exit/s25-manual.sh` walks through all of it and asks what you saw; write
that into `docs/stages/s25.md` under "Manual run".

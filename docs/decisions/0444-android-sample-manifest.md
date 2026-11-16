# 0444: The Android device sample gets a manifest and a Java 17 target

The sample had no `AndroidManifest.xml`, so `assembleDebug` failed in CI since it was added. The manifest declares `mason.sample.MainActivity` as the launcher. The build then failed on mismatched JVM targets (Java 1.8 vs Kotlin 21), so the sample pins both to 17. No wrapper is checked in, so the sample's config and README call `gradle` (CI installs 8.12) instead of a `./gradlew` that does not exist.

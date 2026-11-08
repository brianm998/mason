// SPDX-License-Identifier: GPL-3.0-or-later
// The Android phone app: shares `:core` (pairing, pinning, phone layout, the model) and finds the
// daemon with NSD. NOT in settings.gradle.kts yet: it needs the Android Gradle plugin and an SDK
// to resolve, which the machine this stage was built on did not have wired in (see
// docs/stages/s24.md). To build it, add `include(":android")` and the plugin to the root build.
plugins {
    id("com.android.application")
    kotlin("android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "mason.android"
    compileSdk = 35
    defaultConfig {
        applicationId = "dev.mason.android"
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.0.0"
    }
}

dependencies {
    implementation(project(":core"))
}

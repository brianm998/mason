// SPDX-License-Identifier: GPL-3.0-or-later
rootProject.name = "mason-compose"

pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
        google()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
        google()
    }
}

include(":core", ":app", ":desktop")

// The Android phone app needs the Android SDK and the Android Gradle plugin, so it is in the build
// only when asked for: `-Pmason.android=true` (scripts/exit/s469.sh and the Android CI job do).
// Every other build, including the desktop monitor's, never configures it.
if (providers.gradleProperty("mason.android").orNull == "true") include(":android")

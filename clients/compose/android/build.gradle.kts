// SPDX-License-Identifier: GPL-3.0-or-later
// The Android phone app: shares `:core` (pairing, pinning, phone layout, the model) and finds the
// daemon with NSD. It is in the build only where an Android SDK is found (see settings.gradle.kts).
//
//   gradle :android:assembleDebug :android:testDebugUnitTest
//
// The application id comes from a Gradle property, like the iOS bundle id: pass
// -Pmason.android.applicationId=com.example.mason to build the app under your own id.
plugins {
    id("com.android.application")
    kotlin("android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "mason.android"
    compileSdk = 35
    defaultConfig {
        applicationId = providers.gradleProperty("mason.android.applicationId").getOrElse("dev.mason.android")
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = providers.gradleProperty("mason.version").getOrElse("0.0.0")
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    testOptions { unitTests.isReturnDefaultValues = true }
    packaging { resources.excludes += setOf("META-INF/INDEX.LIST", "META-INF/DEPENDENCIES") }
}

dependencies {
    // `:core` is a desktop-JVM module: its Compose runtime is JetBrains' copy and its JSON jar
    // duplicates the platform's. The app brings AndroidX's runtime instead.
    implementation(project(":core")) {
        exclude(group = "org.jetbrains.compose.runtime")
        exclude(group = "org.jetbrains.compose.collection-internal")
        exclude(group = "org.jetbrains.skiko")
        exclude(group = "org.json")
    }
    implementation("com.squareup.wire:wire-runtime:5.2.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")

    val bom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(bom)
    implementation("androidx.compose.runtime:runtime")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.core:core-ktx:1.15.0")
    // The QR scanner: CameraX for the preview and ZXing (Apache-2.0, no Play services) to read the code.
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.4.1")
    implementation("androidx.camera:camera-view:1.4.1")
    implementation("com.google.zxing:core:3.5.3")

    testImplementation(kotlin("test"))
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}

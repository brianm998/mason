// SPDX-License-Identifier: GPL-3.0-or-later
plugins {
    kotlin("jvm")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.compose")
    id("com.squareup.wire")
}

dependencies {
    api(compose.runtime)
    api("com.squareup.wire:wire-runtime:5.2.1")
    implementation("com.squareup.wire:wire-moshi-adapter:5.2.1")
    implementation("com.squareup.moshi:moshi:1.15.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("org.json:json:20240303")
    testImplementation(kotlin("test"))
}

// The messages come straight from the one schema, so Kotlin cannot drift from it.
wire {
    sourcePath {
        srcDir("../../../schemas")
    }
    kotlin { }
}

tasks.test {
    useJUnitPlatform()
    // Never skip as up-to-date: the test step counts the per-test lines `--info` prints, and a
    // skipped task prints none, which reads as "0 tests ran".
    outputs.upToDateWhen { false }
}

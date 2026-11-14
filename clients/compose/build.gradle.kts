// SPDX-License-Identifier: GPL-3.0-or-later
//
// The Compose monitor (Part 17): the same layout, behaviour and design tokens as the
// SwiftUI client, for macOS, Windows and Linux. `core` is everything that is not drawing
// (the connection, state, navigation, search, the conformance items); `app` is the
// Compose views, the window, and `monitor-tool`.
plugins {
    kotlin("jvm") version "2.1.0" apply false
    id("org.jetbrains.compose") version "1.7.3" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.1.0" apply false
    id("com.squareup.wire") version "5.2.1" apply false
}

// Gradle prints a line per test only when a test task actually runs. A task it skips as
// up to date prints none, and the pipeline's gate then counts zero tests and fails. So the
// test tasks always run.
subprojects {
    tasks.withType<Test>().configureEach {
        outputs.upToDateWhen { false }
    }
}

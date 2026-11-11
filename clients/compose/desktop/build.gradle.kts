// SPDX-License-Identifier: GPL-3.0-or-later
//
// The release build of the monitor window, used by the Makefile (`make build`, `make run`).
// It lives apart from `:app` because Compose's desktop application tasks include a `run`
// that clashes with the `application` plugin `:app` uses for `installDist`.
//
//   proguardReleaseJars  the window and its libraries put through ProGuard, in
//                        build/compose/tmp/main-release/proguard
//   runRelease           start the window from those jars
plugins {
    kotlin("jvm")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.compose")
}

dependencies {
    implementation(project(":app"))
    implementation(compose.desktop.currentOs)
}

compose.desktop {
    application {
        mainClass = "mason.monitor.MainKt"
        // ProGuard reads the JDK's jmods, so this needs a full JDK; the JetBrains runtime
        // Android Studio ships (often Gradle's own) has none.
        // `-Pmason.jdk=/path/to/jdk` picks one (the Makefile finds it); otherwise the JDK
        // Gradle runs on.
        (findProperty("mason.jdk") as String?)?.let { javaHome = it }
        // Skiko loads its native library; recent JDKs warn about that unless allowed.
        jvmArgs += listOf("-Dapple.awt.application.name=MASON", "--enable-native-access=ALL-UNNAMED")
        // The commit the window was built from, so it can tell when it is out of date.
        findProperty("mason.build")?.let { jvmArgs += "-Dmason.monitor.version=$it" }
        nativeDistributions {
            packageName = "MASON"
            packageVersion = "1.0.0"
            modules("java.naming", "java.net.http", "jdk.unsupported")
        }
        buildTypes.release.proguard {
            // The bundled default predates Java 21 class files; 7.8 reads up to Java 25.
            version.set("7.8.2")
            obfuscate.set(false)
            configurationFiles.from(project.file("proguard-rules.pro"))
        }
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//
// The release build of the monitor window, used by the Makefile (`make build`, `make run`).
// It lives apart from `:app` because Compose's desktop application tasks include a `run`
// that clashes with the `application` plugin `:app` uses for `installDist`.
//
//   proguardReleaseJars  the window and its libraries put through ProGuard, in
//                        build/compose/tmp/main-release/proguard
//   runRelease           start the window from those jars
import org.jetbrains.compose.desktop.application.dsl.TargetFormat

// One version for every build: `-Pmason.version=X.Y.Z`, else the workspace version in Cargo.toml.
val masonVersion: String = (findProperty("mason.version") as String?)
    ?: Regex("""(?s)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"""", RegexOption.MULTILINE)
        .find(rootDir.resolve("../../Cargo.toml").readText())?.groupValues?.get(1)
    ?: error("no workspace version: pass -Pmason.version")

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
            packageVersion = masonVersion
            description = "MASON monitor"
            vendor = "MASON"
            // Dmg is not built: the Swift app is the macOS monitor. Each format needs its own
            // OS (.deb on Linux, .msi on Windows with WiX); the release workflow builds them.
            targetFormats(TargetFormat.Deb, TargetFormat.Msi)
            modules("java.naming", "java.net.http", "jdk.unsupported")
            // The `mason://` URL scheme is not registered: Compose registers schemes only for
            // macOS (infoPlist), so it is left out on Linux and Windows.
            linux {
                // Must not clash with the CLI's `mason` .deb.
                packageName = "mason-monitor"
                iconFile.set(project.file("../packaging/icons/mason-monitor.png"))
                menuGroup = "Development"
                appCategory = "Development"
            }
            windows {
                // NEVER CHANGE: Windows uses this to treat a new .msi as an upgrade of the old.
                upgradeUuid = "6f0c5b8e-3d9a-4c47-9a52-7e1b8d2f4a61"
                iconFile.set(project.file("../packaging/icons/mason-monitor.ico"))
                menu = true
                menuGroup = "MASON"
                shortcut = true
                perUserInstall = true
            }
        }
        buildTypes.release.proguard {
            // The bundled default predates Java 21 class files; 7.8 reads up to Java 25.
            version.set("7.8.2")
            obfuscate.set(false)
            configurationFiles.from(project.file("proguard-rules.pro"))
        }
    }
}

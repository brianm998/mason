// SPDX-License-Identifier: GPL-3.0-or-later
plugins {
    kotlin("jvm")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.compose")
    application
}

dependencies {
    implementation(project(":core"))
    implementation(compose.desktop.currentOs)
    implementation(compose.material3)
    implementation("org.json:json:20240303")
    testImplementation(kotlin("test"))
}

// The mascot art is one copy, in the SwiftPM target; Compose reads the same directory as
// resources (icons/, banners/, illustrations/, brand/ at the classpath root).
sourceSets {
    main {
        resources.srcDir("../../swift/Sources/MasonMascot/Art")
    }
}

// `installDist` makes build/install/app/bin/app: the conformance and screenshot tool.
// The window itself runs with `./gradlew :app:run` (or `desktopRun`).
application {
    mainClass.set("mason.monitor.ToolKt")
}

tasks.register<JavaExec>("monitor") {
    group = "application"
    description = "Run the monitor window."
    classpath = sourceSets["main"].runtimeClasspath
    mainClass.set("mason.monitor.MainKt")
    jvmArgs("-Dapple.awt.application.name=MASON")
}

// `bin/mason-monitor` in the installed tree: the window, stamped with the commit it was built
// from (`-Pmason.build=<sha>`) so that it can tell when it is out of date.
val monitorScripts by tasks.registering(CreateStartScripts::class) {
    applicationName = "mason-monitor"
    mainClass.set("mason.monitor.MainKt")
    outputDir = layout.buildDirectory.dir("monitor-scripts").get().asFile
    classpath = tasks.named<CreateStartScripts>("startScripts").get().classpath
    defaultJvmOpts = listOf("-Dapple.awt.application.name=MASON") +
        (findProperty("mason.build")?.let { listOf("-Dmason.monitor.version=$it") } ?: emptyList())
}

distributions {
    main {
        contents {
            from(monitorScripts) { into("bin") }
        }
    }
}

tasks.test {
    useJUnitPlatform()
}

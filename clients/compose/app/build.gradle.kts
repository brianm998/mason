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
    testImplementation(kotlin("test"))
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

tasks.test {
    useJUnitPlatform()
}

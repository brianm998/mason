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

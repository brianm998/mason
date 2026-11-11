// swift-tools-version:6.0
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The SwiftUI monitor (Part 17). `MonitorCore` is everything that is not
// drawing: the connection, the state, navigation, search, the conformance items.
// `MonitorUI` is the SwiftUI views. `MasonMonitor` is the macOS app,
// `monitor-tool` the conformance dump and the screenshot renderer.

import PackageDescription

let package = Package(
    name: "MasonMonitor",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .executable(name: "MasonMonitor", targets: ["MasonMonitor"]),
        .executable(name: "monitor-tool", targets: ["MonitorTool"]),
        .library(name: "MonitorPhone", targets: ["MonitorPhone"]),
        .library(name: "MonitorCore", targets: ["MonitorCore"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.28.0"),
    ],
    targets: [
        .target(
            name: "MasonProtocol",
            dependencies: [.product(name: "SwiftProtobuf", package: "swift-protobuf")]
        ),
        .target(name: "MasonTokens"),
        // The mascot art (Part 17), one copy in Art/ that Compose reads too. Loaded by
        // `Mascot`, which finds the resource bundle beside the executable or in the app's
        // Resources, never through the build-directory path SwiftPM bakes in.
        .target(name: "MasonMascot", resources: [.copy("Art")]),
        .target(name: "MonitorCore", dependencies: ["MasonProtocol", "MasonTokens"]),
        .target(name: "MonitorPhone", dependencies: ["MonitorCore", "MasonTokens", "MasonMascot"]),
        .target(name: "MonitorUI", dependencies: ["MonitorCore", "MasonTokens", "MasonMascot"]),
        .executableTarget(name: "MasonMonitor", dependencies: ["MonitorUI", "MonitorCore", "MasonMascot"]),
        .executableTarget(name: "MonitorTool", dependencies: ["MonitorUI", "MonitorCore", "MasonMascot"]),
        .testTarget(name: "MonitorCoreTests", dependencies: ["MonitorCore"]),
        .testTarget(name: "MasonMascotTests", dependencies: ["MasonMascot"]),
        .testTarget(name: "MonitorUITests", dependencies: ["MonitorUI", "MonitorCore", "MasonProtocol"]),
    ],
    swiftLanguageModes: [.v5]
)

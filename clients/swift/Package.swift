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
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "MasonMonitor", targets: ["MasonMonitor"]),
        .executable(name: "monitor-tool", targets: ["MonitorTool"]),
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
        .target(name: "MonitorCore", dependencies: ["MasonProtocol", "MasonTokens"]),
        .target(name: "MonitorUI", dependencies: ["MonitorCore", "MasonTokens"]),
        .executableTarget(name: "MasonMonitor", dependencies: ["MonitorUI", "MonitorCore"]),
        .executableTarget(name: "MonitorTool", dependencies: ["MonitorUI", "MonitorCore"]),
        .testTarget(name: "MonitorCoreTests", dependencies: ["MonitorCore"]),
    ],
    swiftLanguageModes: [.v5]
)

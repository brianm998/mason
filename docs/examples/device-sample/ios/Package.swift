// swift-tools-version:5.9
// SPDX-License-Identifier: GPL-3.0-or-later
import PackageDescription

let package = Package(
    name: "SampleApp",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [.library(name: "SampleApp", targets: ["SampleApp"])],
    targets: [
        .target(name: "SampleApp"),
        .testTarget(name: "SampleAppTests", dependencies: ["SampleApp"]),
    ]
)

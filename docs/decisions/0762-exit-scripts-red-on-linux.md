# 0762: s591 is skipped on Linux

The Swift package builds a SwiftUI target, so `swift test` cannot build on Linux. The exit script skips
(exit 0) when the host is not Darwin; macOS still runs the disk tests.

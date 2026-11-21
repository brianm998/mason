# 0748: swift-clients runs on macos-latest

The monitor needs macOS (SwiftUI, AppKit); `swift build --build-tests` fails on the default `ubuntu-latest`.
Decision: set per-suite `runs_on = "macos-latest"` on `swift-clients`. Judging is unchanged.
Needs owner approval as a trust-file change (owner asked for remote runs on 2026-11-20).

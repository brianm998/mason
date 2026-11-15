# 0448: iOS CI is its own workflow with a shared setup action

A path filter is per workflow and `ci.yml` must run on every change, so `monitor-ios` is in `.github/workflows/ios.yml`, filtered to `clients/swift/**` and its own files, with its own concurrency group. Xcode pin, XcodeGen and caches sit in the composite action `.github/actions/ios-setup` so the TestFlight job can reuse them. This touches protected `.github/**`; owner approval is needed.

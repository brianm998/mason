# 0107: App icon from the existing mascot art

The icon is the copy of `brand/app_icon.png` already bundled with the mascot art (it lives under `clients/swift/Sources/MasonMascot/Art/brand`), so the Mac and Compose clients add no second copy to drift; only the iOS file is a copy, since a future Xcode project needs it beside its plist. The bundle script resizes with `sips` (always on macOS) before `iconutil`. The unbundled Swift monitor sets the icon only when the Info.plist has no `CFBundleIconFile`. `clients/features.toml` is a protected path: only a feature was added, no check weakened.

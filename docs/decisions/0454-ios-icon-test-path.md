# 0454: the iOS icon test reads the asset catalog path

The iOS icon lives in `clients/swift/ios/MasonIOS/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png` since s446. `testTheIosIconStaysOpaqueAndFullBleed` follows it. The icon itself is unchanged and must stay opaque and 1024 px wide (0118); the assertions are unchanged.

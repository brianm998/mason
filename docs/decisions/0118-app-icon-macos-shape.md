# 0118: A macOS-shaped app icon beside the full-bleed one

macOS draws an icon exactly as given: no rounded mask, no shrinking. The full-bleed `brand/app_icon.png` (1024 x 1024, opaque) therefore showed as a hard square about 24% wider than its neighbours. iOS and Android mask icons themselves, so they keep the square.

`brand/app_icon_macos.png` is the same artwork scaled into Apple's 824 x 824 rounded rectangle (continuous corners, radius about 185) centred on a transparent 1024 canvas, with a soft shadow in the margin (30% black, 28 px blur, 12 px down). It is made by `scripts/gen-app-icon-macos.swift` (Swift and CoreGraphics only, run as `swift scripts/gen-app-icon-macos.swift` from the repository root). **Regenerate it whenever `app_icon.png` changes**; both files are committed so builds never run the script.

The corner is a Bezier approximation of Apple's squircle, not Apple's exact curve. The art is scaled, not cropped, so only the corners lose pixels.

Used by: `bundle-monitor.sh` (the `.icns`), `Mascot.macAppIconData()` for the unbundled Swift monitor, and Compose `Mascot.appIcon()` on macOS (other desktops keep the square). It is a new entry in `clients/mascot.toml` and both generated manifests. `AppIcon-1024.png` for iOS is untouched and must stay opaque. `clients/features.toml` is unchanged. Supersedes the icon source named in 0107.

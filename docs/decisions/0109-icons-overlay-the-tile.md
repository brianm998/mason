# 0109: State icons overlay the count tile

DESIGN Part 17 sizes a tile by its label and text size only. An inline icon beside a count would widen short tiles at two digits, so the icon is an overlay offset outside the tile's top-left corner and takes part in no layout (SwiftUI `.overlay`; Compose a child measured at 0 x 0 and placed at a negative offset).

- Sidebar rows: the icon ends the time and cells line; the model text has the lowest layout priority and truncates, time and cells are fixed. The title line is unchanged.
- Sizes come from the existing `mascot_icon_small` / `mascot_icon` tokens times `theme.scale`; the Compose icon always uses the 3x file and is scaled down. The phone scales them with Dynamic Type, as it has no theme.
- A new Swift test target, `MonitorUITests`, holds the tile-width test; `Package.swift` is not a trust file.
- Not changed: `Items.swift` / `Items.kt` and all text labels, so accessibility trees are unchanged.

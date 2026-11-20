# 0638: Offscreen hosts force a draw

Decision: test hosts call cacheDisplay into a bitmap after layout instead of ordering the window in. SwiftUI evaluated zero bodies on the CI runner with layout alone. Nothing appears on screen, and no assertion is weakened.

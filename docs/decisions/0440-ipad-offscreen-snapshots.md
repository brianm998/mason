# 0440: iPad layout is checked with ImageRenderer from a shared MonitorUI helper

`monitor-tool snapshot --ipad` and the tests both call `IPadSnapshot` in MonitorUI, which renders `RootView` with `ImageRenderer` (scale 2) at 1366×1024 and 1180×820 points, light and dark. Nothing opens a window or a simulator. The top-bar fit check reuses `TopBarFitTests`' measurement (least width via a hosting controller at width 0).

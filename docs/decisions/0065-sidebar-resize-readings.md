# 0065: Sidebar resize, readings

Stage 79 (task 55). Where the task left room, these are the readings taken.

1. **Stored unscaled, clamped both ways.** The width stored under `sidebarWidth` is the base,
   before text zoom, clamped to 220..640 when set and when read back (a hand-edited or old
   value cannot escape the limits). Drawn width is base * min(scale, 1.5), the factor the
   fixed width already used.
2. **The panel gives way first, in drawn points.** `fit(desired, available, scale)` is
   `max(220 * factor, min(desired, available - 360 - 8))`. The main view's 360 and the handle's
   8 are not scaled; at a very narrow window the panel's scaled floor wins over the main
   minimum. The stored value is not changed by a narrow window.
3. **Hand-written handle in both clients**, not `HSplitView`, so the limits, the fit rule and
   the persistence are the same code path in `MonitorModel`. The handle is 8 wide
   (`sidebar_handle`) and draws a 1pt line; the pointer is read in the row's coordinates and
   the line's centre (half the handle) is subtracted so the line follows the pointer.
4. **One list for both views.** `sidebarEntries` (MonitorCore, Compose core) is the scrolling
   part only: section headers, expanded rows, a rule, the four page links. Home and search are
   pinned above it. `Items.swift` and `Items.kt` are unchanged. The Swift section view was
   split so its rows are separate entries, as in Compose; row accessibility elements are
   unchanged.
5. **Gate file.** `clients/features.toml` gains one feature row; it is a self-hosting gate file
   and the task asks for it, so it waits for owner approval.

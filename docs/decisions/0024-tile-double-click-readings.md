# 0024: Tile double-click, the choices the ticket left open

Task #24. Each is the reading taken; change one and the others stand.

- **Click count source.** Swift reads `NSApp.currentEvent?.clickCount` inside the one `onTapGesture`. Compose counts presses inside the AWT multi-click interval (`awt.multiClickInterval`, 500 ms if unknown) in a `pointerInput` and fires on release, as `clickable` did. Neither waits to tell a single click from a double.
- **One model path.** `tileClicked(state, clickCount)` reveals on every click and calls `revealAndOpenIfOne(state)` when the count is 2. The views only report the count.
- **Open rule.** `revealAndOpenIfOne` acts on the section's rows at that moment, not the tile's count. It opens only when connected and the section holds exactly one row. Zero rows: the reveal only. Disconnected: nothing at all (not even the reveal) from the double-click path; a single click is unchanged.
- **Search.** The lone task opens even if the search hides it (the owner saw the count on the tile). The search text and flags are left alone.
- **Tooltip.** `tileTooltip` appends "Double-click to open it" client side when connected and the count is exactly 1.
- **Reliable scroll.** Swift clears `scrollTarget` and runs `scrollTo` on the next main-queue turn, so the expanded rows are laid out first. Compose clears `scrollTarget` before `animateScrollToItem`, so a cancelled animation cannot leave it set.
- **Not done: conformance "open" action and a `tile` tool command** (planned, left out). The conformance item set is unchanged, so daemon and clients stay in step.

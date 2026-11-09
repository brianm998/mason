#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s79 by hand: prints the checklist. Start `masond --demo` with enough tasks for the list to
# scroll and run each monitor, then go through these. Nothing here is checked by a tool.
cat <<'EOF'
Run both monitors (Swift app, Compose app) against masond --demo, one at a time:

 1. Hover the divider: a horizontal-resize cursor shows; the line stays thin.
 2. Drag it right and left: the panel follows the pointer without jitter, rows re-wrap,
    the main view reflows live.
 3. Drag past 220 and past 640: the panel stops at each limit.
 4. Shrink the window: the panel gives way until the main view is 360 wide.
 5. Quit and relaunch: the width is kept. With no stored width it is 300.
 6. Change text zoom: the panel keeps its proportion.
 7. Scroll to the bottom of the list: Home and search are still at the top and work.
 8. Click a Home tile: its section comes to the top of the list, below Home and search.
EOF

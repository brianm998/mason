# 0533: keep the UI-thread watchdog on in release builds

**Decision.** Both monitors run the watchdog in every build at a low rate (250 ms ping, 100 ms threshold) and show the last-10-minutes numbers on the Health page. The fast 20 ms / 50 ms setting stays for debug builds and `MASON_WATCHDOG=1`.

**Why.** The 2026-11-16 slowdown (a monitor at 100% CPU for 8 hours) was found only with `sample`, because the owner's release monitor recorded nothing. One ping per 250 ms on a utility queue costs next to nothing.

**Not done.** The numbers are not sent to the daemon; a later ticket may.

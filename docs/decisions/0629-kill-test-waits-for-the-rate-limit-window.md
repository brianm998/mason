<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0629: the kill test waits for the rate-limit window

Status: accepted (s636).

Context: the test asserted "ok" before the stored rate-limit snapshot had passed its reset.

Decision: the test waits for the window to clear rather than moving the snapshot, because the snapshot is what the provider reported.

Consequence: the test takes up to HOLD_S longer when landing is fast; production code is unchanged.

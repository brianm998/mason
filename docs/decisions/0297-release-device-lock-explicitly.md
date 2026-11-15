# 0297: Release the device lock explicitly on drop

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

A device lease is a flock, which belongs to the open file description, not the descriptor. Closing our fd does not release it while a dup or forked child holds another. `DeviceLease::drop` therefore calls `unlock` (LOCK_UN) first, which releases it for all copies. The flaky daemon test needed no change.

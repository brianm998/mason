<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0637: assert on task ids, not on a Debug string with timestamps

Status: accepted (s644).

Context: the ten-compiles test checked that "510" no longer appeared in the Debug text of the wait view. That text includes epoch-ms times, which can contain "510".

Decision: check the `task` field of every holder, blocker, chain party and hold entry instead. The check is the same in intent and does not depend on the clock.

Consequence: none for production code.

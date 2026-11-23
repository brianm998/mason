<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1032: s249 and s782 red again on Linux

## Decision

Make only the change that is safe without the failing log: s782 clears inherited MASON_EXIT_* variables. Leave the cause open and do not claim the red is fixed.

## Ruled out

The s1022 theory (shard leak into s249's temp repo, s782's quiet text) did not hold on a0f67a2.

## Next

Read the failing lines of run 45143216377 (see docs/stages/s1039.md), then make the smallest fix that keeps the assertion strict. s782.sh is protected and needs owner approval.

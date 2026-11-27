<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1212: bounded manual merge rounds

A run_check whose base fold conflicts asks for a hand merge only `MANUAL_MERGE_ROUNDS` (3) checks in
a row. After that the agent is told to stop merging and let the landing queue fold the base, where a
conflict is already bounded by `CONFLICT_ROUNDS`. A true restart from a fresh develop tree was
rejected: it needs a new tree-building mechanism. A clean or merged fold resets the count.

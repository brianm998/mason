#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s160 manual exit test, for the owner on the macOS monitor.
cat <<'TXT'
1. Open an idle task that has a worktree; click Open session. A Terminal window opens running
   `claude --resume <sid>` in that worktree (a fresh session when there is none).
2. Click it on a task whose step is running: the monitor asks first; Open anyway opens it.
3. Make the terminal fail (for example move Terminal.app aside): the page says
   "Copied `...`; paste it in a terminal."
TXT

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1210: a pool wait is not stuck:task

A step waiting for a GitHub runner pool unit is skipped by stuck:task. overloaded:slots reports the wait after 30 min and stuck:wait after 2 h.

Not chosen: a history-based baseline for landing. A queued landing has no finished step run, so the baseline would still be 0 on the first run.

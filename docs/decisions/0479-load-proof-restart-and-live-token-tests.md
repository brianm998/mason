<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0479: Restart-limit and live-token tests do not measure daemon latency

Status: accepted

`restart_limit` asserted that the host ended before `restarted + LIMIT_MS`. The daemon stops a
late-attached host at attach time, so that bound measures how long the daemon took to start and
fails on a loaded machine. It now checks that the timer stopped the host (a nonzero exit signal,
since the command sleeps far longer than the limit) and keeps the lower bound from `started_ms`.
Restart-begins-no-new-count stays covered by the unit tests on `remaining_at`.

`a_running_steps_cell_gains_tokens_before_it_ends` used a 9 s sleep as its window. It now uses a
develop turn that waits (bounded to about 120 s) for a sentinel file the test writes after seeing
both messages' tokens. `slow_develop` is unchanged for the other tests.

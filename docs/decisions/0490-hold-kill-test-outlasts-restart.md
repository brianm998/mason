<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0490: The hold kill test's hold outlasts a restart

Status: accepted

The test stored a usage-limit hold of 120 s, then killed and restarted the daemon. The restart runs
a git fetch and process launches with no bound, so on a busy or stalled machine it could outlast
120 s. The hold then lapsed, the tasks started their develop turn, and the "still blocked"
assertion failed. The hold is now 3600 s. The test already moves the stored hold's end to two
seconds from now before waiting for the tasks to land, so its runtime does not grow. The flake was
not reproduced here; the cause comes from reading the code.

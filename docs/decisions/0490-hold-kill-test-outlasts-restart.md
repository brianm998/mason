<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0490: The hold kill test's hold outlasts a slower restart

Status: accepted

The test stored a usage-limit hold of 120 s, then killed and restarted the daemon. The restart runs
a git fetch and process launches with no bound, so on a busy or stalled machine it could outlast
120 s. The hold then lapsed, the tasks started their develop turn, and the "still blocked"
assertion failed. The hold is now 240 s. It cannot be much longer: the fake agent's five-hour
window stays rejected until its reset, so moving the stored hold's end earlier does not lift the
account, and the test waits out the rest (inside its 300 s landing waits). A first try at 3600 s
timed out there. The flake was not reproduced here; the cause comes from reading the code.

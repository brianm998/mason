<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0490: The hold kill test's hold outlasts a slower restart

Status: accepted

The test stored a usage-limit hold of 120 s, then killed and restarted the daemon. The restart runs
a git fetch and process launches with no bound, so on a busy or stalled machine it could outlast
120 s. The hold then lapsed, the tasks started their develop turn, and the "still blocked"
assertion failed. Lengthening the reported hold does not help: the fake agent's five-hour window
stays rejected until its reset, so the test would wait that long (3600 s timed out, 240 s ran
about 270 s and failed once). Instead the test now reports a 60 s hold and, after checking its
reset, pushes the stored hold's end an hour out before the kill, so the restart can take any
time. After the restart check it moves the end to two seconds out as before, and the window
lapses within its 60 s. The flake was not reproduced here; the cause comes from reading the code.

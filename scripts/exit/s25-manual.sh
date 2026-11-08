#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 25 manual run: the sample app through a real iOS simulator, a real Android emulator and a
# real phone. This script only prints the steps; the owner does them and writes what was seen
# into docs/stages/s25.md under "Manual run". Not run by run-exit-scripts.sh.
cat <<'TXT'
Stage 25 manual run. You need a Mac with Xcode (a simulator), the Android SDK (an emulator
created in Device Manager) and at least one phone (iPhone, or Android with USB debugging).

 1. Copy docs/examples/device-sample to a new git repository (see its README) and register it.
 2. bash scripts/exit/s25-real.sh          -> what does `mason targets list` show? Is it right?
 3. File a task touching ios/. Which simulator did it use? Did MASON boot it, and shut it down
    afterwards (and leave alone one that was already running)?
 4. File a task touching android/ with only an emulator running. Same questions.
 5. Plug in the phone, unlock it, trust the computer. File a task. Did it run on the phone?
 6. Unplug the phone and file another. Did it wait with "environment problem, not the task's
    fault", raise an `environment` alert, and show no failure? Plug it back in: did the task
    resume within a couple of seconds?
 7. If the signing certificate or provisioning profile can be made to expire (or use an old
    profile), did the red become a wait and an alert, not a failure?
 8. Write what you saw, including anything surprising, under "Manual run" in docs/stages/s25.md.
TXT

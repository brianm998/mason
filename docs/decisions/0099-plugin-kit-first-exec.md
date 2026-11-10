# 0099: prime scripts in the kit tests, do not change the kit timeout

Cause: on macOS the first exec of a newly written executable can take over 10 s (syspolicyd scanning). The kit tests write fresh copies of the example plugin, so the first kit call (initialize) timed out at 10 s with no plugin code involved.

Fix: the tests run each script once beforehand (`prime`), untimed, with stdio null. The cost moves outside every timer.

Not changed: the kit timeout. It is a knob users and the conformance CLI pass in, and the stall is per fresh file, not a property of plugins; raising it would slow every genuine hang report and only hide the stall. `T` (10 s) and `SHORT` (6 s) stay as they are.

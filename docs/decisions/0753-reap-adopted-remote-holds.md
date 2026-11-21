# 0753: Reap adopted remote holds against GitHub

Status: accepted (task #787)

Decision: holds adopted at start are checked against GitHub periodically. A finished run is
marked `released` rather than `collected`, so its units are given back but a later attach still
fetches the real result from GitHub instead of a result made up from the conclusion. Runs no one
can collect (task has no open run, superseded check) are cancelled and their records removed.

Alternatives: marking `collected` with a code from the conclusion (loses output, can be wrong);
hooking every task-end path (many paths; the periodic pass over adopted holds covers the leak,
which only arises from adopted holds). No protected file is touched.

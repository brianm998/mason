# 0911: exit-scripts remote runs skip Swift and Compose; base reds excuse exit scripts

Remote exit-scripts runs get `MASON_SKIP_SWIFT=1` and `MASON_SKIP_COMPOSE=1`, mirroring ci.yml (decisions 0443, 0622). `toolchains()` is unchanged.

JDK 21 was rejected: installing java would make s13 run remotely, and s13 launches the GUI app on a Linux runner with no `MASON_NO_GUI`.

`red_base::failing_tests` now names exit scripts from `FAILED: scripts/exit/sNN.sh` and `FAIL sNN`, so `judge` gives RedOnBase when the base is red with the same scripts, and Blame when the base is green or unknown.

s07 and s09 are not explained by the evidence and are neither changed nor excused in code. They run because their text names `-p mason-definition`; a failure is #896's own, or a base red now covered above.

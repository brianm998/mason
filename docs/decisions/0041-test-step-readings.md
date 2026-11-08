# 0041: Readings made while making a test step judge the change (s61)

- **Rerunning a red is not a weaker gate, as long as a pass needs a complete green run.** A broken change is red on every run and still fails. A test that fails only sometimes passes either way, with or without reruns, and a rerun only spares the task a red round (and an agent session) for something its change did not cause. Each run is a complete run of the same command on the same tree, and the step is judged only by a completed run.
- **Opt in, per project.** Doubling the time of a real red is a price only a project with flaky, slow suites wants to pay. MASON's own pays it; the default is 0. The limit is 3.
- **Only exit-status reds are rerun.** A timeout or a signal is about time, not flakiness, and a rerun would double the wait. A script that wrote `passed: false` has given its own verdict.
- **The base is folded in at the test step, not only at develop.** The tests then run on what would land, the same tree landing merges. A fix to a broken test that lands on the base reaches a waiting task at its next round, not only after another develop round. A conflict at that point is a red round, with the conflict note; landing already treats a conflict the same way.
- **`MASON_HELD` comes from the step's `uses`, not from the lease.** That is what the definition promises the step holds, and it is known before the command starts. Classes a suite environment adds are not listed; only `uses` are.
- **Numbers:** s61 and 0041 follow s60 and 0040, above everything MASON had reserved.

# 0669: the fast-tier test orders the idle debt pass

Cause (reasoned, not reproduced): the test daemon starts its idle debt pass about 60 s after start
(`spawn_debts`, suites.rs). That pass calls `pay` on the first task's open `step:test` skip debt in
the same repo, under the same compile pool and repo lock, while the test creates its second task.
The test waited only for `running_batches == 0` and `landed`, never for the debt.

Decision: after the first task is on main, the test pays that debt itself (`suites::pay`, safe if
already paid) and asserts it is no longer open. No timing assertion, no assertion removed.
The pipeline's repeat runs are the evidence.

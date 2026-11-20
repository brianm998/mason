# 0685: rerun and bisect a red batch on its failing tests

Status: proposed (verdict logic in `landing.rs`; waits for the owner's approval)

**Decision.** When a batch's red run named failing tests and each red suite has a `rerun` template, the red ladder reruns only those tests (serially) and the bisect builds each subset and runs only those tests. Otherwise (no names, build failure, timeout, no template, floor or lint red) it runs whole suites as before.

**Why.** Batch #421 spent ~2 h of the queue on six whole-suite runs to find a test that fails in 3 s.

**Kept.** A green filtered rerun is confirmed by the whole rerun; the remainder of a bisected batch gets a whole-suite run on the next round. Nothing lands on a filtered run alone, so what is verified before landing does not change.

**Cost.** A bisect subset that passes the failing tests but would fail another test is called green; that red is found by the remainder's whole run, and its tasks go back then.

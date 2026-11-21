# 0732: the landing_yield test is red at ca5406fb

Status: proposed, needs the owner's approval.

`crates/daemon/src/landing.rs` is in `PROTECTED` (`crates/daemon/src/protect.rs`), so any fix there needs the owner's approval, as decision 0698 did for the code this test covers. The test's assertions and timeouts are not changed.

The cause is in `crates/daemon/src/suites.rs` (also PROTECTED), not `landing.rs`: the immediate rerun of a red suite is a second run before the batch can yield. While an update drains, a landing batch's run skips it; the next batch's own ladder reruns what is red. The cost: a flaky red seen while draining is kept as red for the next batch instead of being cleared by the immediate rerun.

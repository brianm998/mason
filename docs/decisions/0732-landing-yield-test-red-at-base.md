# 0732: the landing_yield test is red at ca5406fb

Status: open.

`crates/daemon/src/landing.rs` is in `PROTECTED` (`crates/daemon/src/protect.rs`), so any fix there needs the owner's approval, as decision 0698 did for the code this test covers. The test's assertions and timeouts are not changed.

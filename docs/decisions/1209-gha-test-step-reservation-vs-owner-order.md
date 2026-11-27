<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1209: over-bound test steps on the gha pool versus the owner's order

Task #1227 proposes that when a develop test step has waited past its stuck:wait bound, one gha unit
is reserved for the oldest such waiter before another landing or check is admitted.

The current order (landing, check, test step, debt) was set by the owner on 2026-11-21 (see 0750).
The aging rule lifts only within a kind. The proposal conflicts with that order. A code change would
touch protected paths: `crates/scheduler/**` and `crates/daemon/src/sched.rs`.

Position discrepancy: position counts all ranked-ahead requests on the same resource
(`plan.rs:963`, `sched.rs:1988`), while depth is per class; 0605 says "in class". A test step behind
landings and checks therefore breaches its bound by design.

## Decision needed from the owner

- (a) Keep the order; make the wait explanation say the bound is breached by design. No code change. **Recommended.**
- (b) Reserve one unit for the oldest over-bound test step, as proposed.
- (c) Lift an over-bound test step above checks but not landings.

Nothing under `crates/scheduler/**` or `crates/daemon/src/sched.rs` changes until the owner picks (b) or (c).

# 0319: Deferred is its own state, and approval carries the priority

The owner needs to put a proposal aside without rejecting it. A separate `Deferred` state keeps Proposed meaning "waiting for a decision now", so alerts and the needs-you count stay honest. It has no timer: only the owner brings it back (approve, reject, undefer).

A priority chosen at approval is applied as the same event `task priority` emits, right after `Approved`, rather than a field on `Approved`. This leaves the existing `Approved` payload and golden event logs untouched, so old logs replay identically.

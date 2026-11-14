# 0239: releasing an outside slot is not synchronous with the drop

## Context

`a_slot_held_outside_the_broker_is_not_granted_again` failed in full lib runs. Slots are flocks, freed only when all fd copies close; a parallel test's forked child can briefly hold one.

## Decision

Retry `lease_poll` in the test, bounded by attempt count. The product is unchanged: the kernel is the truth, and loosening the adoption logic would weaken the gate.

## Consequences

The test tolerates a short delay in release. A real failure to ever release still fails after 500 attempts.

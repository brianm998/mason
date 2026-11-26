# 1127: exit scripts s723 and s727 wait for what they depend on

## Context

Both scripts assumed an order the daemon does not promise (#1160), and turned unrelated changes
red. s723 read the fake GitHub's request log once, right after the pool lease. s727 expected
`task run-on` to arrive before the test step of a task that nothing held back.

## Decision

- s723 waits, bounded, for the lookup to appear in the log.
- s727 files the task after one that has not landed, and makes that certain: the earlier task's
  local suite waits on a hold file until the later task has been set to local.

## Consequences

- No assertion is removed or loosened.
- `scripts/exit/**` is a protected path (decision 0019), so this change, like any edit there,
  needs the owner's approval before it lands.

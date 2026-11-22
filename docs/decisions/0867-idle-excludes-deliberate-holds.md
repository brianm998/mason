# 0867: landing idle excludes deliberate holds

## Context
`landing_idle_ms` flagged 10 ready tasks idle for 5 hours.

## Decision
Drain, unreadable configuration and a full freeze reset the idle spell; those have their own alerts.

## Consequences
The detector fires only when the queue could start a batch and does not.

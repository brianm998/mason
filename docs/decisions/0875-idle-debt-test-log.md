# 0875: owing a debt in the idle-debt tests clears the evidence

## Context
Three tests counted runs of a suite script after `owe`; the suite was reused from evidence and never ran.

## Decision
`owe` also records the landed commit on each debt and deletes the `evidence` rows (landed on develop; this task records why).

## Consequences
The daemon is unchanged; no assertion changed.

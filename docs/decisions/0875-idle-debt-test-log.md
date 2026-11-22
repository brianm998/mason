# 0875: owing a debt in the idle-debt tests clears the evidence

## Context
Three tests counted runs of a suite script after `owe`; the suite was reused from evidence and never ran.

## Decision
`owe` also deletes the `evidence` rows, so the tree is not already proven.

## Consequences
The daemon is unchanged; no assertion changed.

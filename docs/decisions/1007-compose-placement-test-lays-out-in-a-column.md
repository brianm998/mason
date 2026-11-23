# 1007: compose placement test lays out in a column

## Context

`aProjectSetForItselfCanBeChanged` failed at 7f90d5a714: the click on `settings:remote:global:remote` landed on `settings:remote:project:favor-local`. The test root stacks top-level children, and `RemotePlacementBody` emits several without a layout of its own.

## Decision

Fix the harness: the test wraps the body in `Column`, as `Section` does in Pages.kt. Rejected: wrapping inside `RemotePlacementBody`, which would change the production tree for a test-only problem.

## Trust

No protected path is touched.

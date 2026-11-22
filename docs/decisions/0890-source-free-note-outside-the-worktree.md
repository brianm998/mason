# 0890: the source-free note lives outside the worktree in tests

## Context

The test wrote its note inside the repo worktree; the untracked file made the later submit fail the dirty-tree check.

## Decision

Test fixtures keep notes outside the repo, as production does (task directory). The refusal is not weakened.

## Consequences

The test passes with unchanged assertions; the temp directory is left to the OS to clean.

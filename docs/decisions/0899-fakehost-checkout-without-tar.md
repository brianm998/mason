# 0899: the fake host checks out a PR head without tar

## Context
#706, #709 and #828 left `git archive | tar -x` in `checks_of`. On the Windows runner tar fails to open the path, so the run is Red and never reaches Merged.

## Decision
Materialise the tree with `git read-tree` and `git checkout-index` against a per-sha index outside the work tree, arguments passed without a shell.

## Consequences
No tar on any host; Unix behaviour is unchanged. A failed checkout is still not cached.

# 1034: reuse a run only for its own tree

## Decision

A remote run record and a local step host carry the key `sha256(tree_key, command)`, and are reused only on an equal key. A test step records each attempt id with its key and, after a restart, attaches to the latest same-key attempt and cancels the others.

## Why this only tightens verdict logic (`suites.rs` is protected)

A result is handed back only when it is for the same tree and command; before, it was handed back for the same id. Nothing that was refused is now accepted. The cost is a rerun when git cannot say what the tree is.

## Not done

No end-to-end test of the suites-level recovery; see docs/stages/s1041.md.

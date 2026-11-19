# 0609: doc tests run in their own cargo group

Status: accepted (s616).

Context: cargo rejects `--doc` combined with `--lib`, `--bins` or `--test`, and the rendered affected groups combined them, so the cargo-workspace suite went red.

Decision: doc tests run as a separate cargo invocation (`-p NAME --doc`), never combined with unit or integration targets.

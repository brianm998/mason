# 0734: Map `scripts/test-shard.py` to `cargo-workspace`

**Chosen.** The shard runner maps to `cargo-workspace`, like `scripts/test-parallel.py`, because cargo test runs through it.

**Not chosen.** Excluding it from the unmapped check would loosen the gate.

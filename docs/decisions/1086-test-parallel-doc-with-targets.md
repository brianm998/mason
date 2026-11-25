# 1086: test-parallel refuses --doc with a target selector

scripts/test-shard.py silently runs nothing for `--doc` beside a target selector. scripts/test-parallel.py instead exits non-zero with a message naming the conflict, before running cargo: the command asks for two things cargo will not do together, and the caller should hear that rather than get a result for one of them or for neither. Each alone behaves as in test-shard.py.

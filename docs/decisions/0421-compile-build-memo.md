# 0421: remember a passed build per tree and worktree

Decision: memoise a passed build, keyed on tree, build command and worktree, in the same table as the test-step memo. The worktree is in the key because a build's output is on disk there; a hit elsewhere would run tests with nothing built.

Rejected: raising `compile_jobs` or loosening the CPU gate. That is machine policy and the owner's call.

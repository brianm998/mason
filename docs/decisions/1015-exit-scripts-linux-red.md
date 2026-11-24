# 1015: exit scripts red on Linux CI

## Decision

s249 runs its helper with `MASON_EXIT_SHARD` and `MASON_EXIT_PLAN` unset, since the helper's shard logic is right for the real repo but wrong for a temp repo. s782 matches `next: (when )?quiet for`, because the text depends on whether the machine is busy. No assertion is removed and no timing assertion is added.

# 1208: a full-CI run starts only under a share of the gha pool

Status: accepted (s1218)

## Context

`mason CI: full` jobs held 15 of the gha pool with 8 queued while task steps waited. The 0799
ranking only orders MASON's own requests; the full-CI jobs are GitHub jobs that MASON adopts as
other holders, so once dispatched they cannot be outranked.

## Decision

A non-forced full-CI dispatch (`crates/daemon/src/ci_schedule.rs`, `full_start_deferred`) is
deferred while the ledger's total `gha` use is above `gha_full_share` percent of the `gha` limit
(default 50), or while any task request for gha units waits (debt payments and the scheduled full
test's own requests do not count). A deferral writes no `full_tests` row, leaves `last_run` and
`runs` unchanged, and retries on the next tick. `mason schedule run --force` ignores it.

`mason resources limit gha_full_share N` (0 to 100) sets the share. It is not a pool. The gha JSON
carries `full_share_pct` and `full_deferred`, and the CLI gha line shows both.

## Not done

Capping a run after it started (`strategy.max-parallel` in `.github/workflows/ci.yml`) is not
done: that file is protected and needs owner approval.

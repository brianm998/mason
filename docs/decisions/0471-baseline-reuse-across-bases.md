# 0471: Reuse a baseline across bases that do not touch the suite

Status: accepted (s478), pending owner approval of the `suites.rs` change.

Context: develop moves ~120 times a day and the baseline cache was keyed on the exact commit, so
63 full-suite baselines ran in one day, many after README-only commits.

Decision: on a miss, reuse the newest stored baseline of an ancestor base when the suite's
definition hash is equal and no changed path is selected by the impact map for that suite. Unmapped
paths and `.mason/` paths always count as touching. Narrowed runs are never cached or reused.

Consequences: a wrong impact map could reuse a stale baseline; the default (unmapped = touches) is
the safe side. The hash covers the suite and its environment, not other definition parts (these
reach the suite only through them). Migration 42 is additive.

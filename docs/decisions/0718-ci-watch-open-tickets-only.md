# 0718: CI-watch banners show open tickets only

Status: accepted.

A banner means "this needs you". A red job whose ticket landed and that no finished run has
confirmed yet does not, so the watch shows it on one quiet line of its own kind
(`ci_watch_quiet`) instead of a `ci_watch` banner. A job whose ticket is open gets one banner
per ticket and shard group. A job red again after the landing is not held back, as before.
A job absent from `agreement + 2` consecutive finished runs of its workflow is forgotten,
with its freeze: it was removed or renamed.

The quiet line is a banner of another kind rather than a new field of the header, so the
protocol schema and the generated client code stay as they are.

Not verified: the clients' drawing of the new kind.

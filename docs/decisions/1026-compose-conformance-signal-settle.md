# 1026: compose conformance settles on a signal, not a quiet window

## Context

`CI: full/monitor-compose` went red on develop: step 2 of `scripts/exit/s13.sh` found 89 differences between the Compose dump and the Rust expected dump, in both directions (rows a step ahead at some checkpoints, a step behind at others) and missing usage banners at checkpoint 12. Both dump sides decided "settled" by a wall-clock quiet window (1.5 s), which a loaded runner outruns.

## Decision

"Settled" is a signal. The Compose tool and the Rust `settle()` ask the daemon for its seq (`GET /status`) over the same ordered connection and are settled when the screen's applied view seq covers it. The daemon sends no view whose content did not change, so when the applied seq stays below the daemon's, three consecutive round trips must agree on the seq with nothing new applied, and (Compose) every subscribed kind must have been applied at least once. A 60 s limit fails with a message naming what was awaited; it never decides.

Rejected: a longer quiet window (decision 0763 rejected it: it only moves the failure to a slower runner) and a looser `compare()` (it would hide real differences).

Decision 1070 gave the Rust `settle()` a 5 s quiet fallback. It is kept (`should_leave`, `QUIET_WINDOW_MS`), but in the Rust capturer the fallback now needs both: the 5 s window has passed and the three round trips agree. The window alone no longer ends the wait. Once the target is reached, the Rust side also keeps the base's 300 ms trailing quiet (`trailing_done`), so later views of the same step are taken in.

## Banners

`seed_usage` takes `now` once and every reading and reset is relative to it, and the banner text depends only on those values, so two dumps taken seconds apart show the same banners. The checkpoint-12 differences were the dump being taken before the usage views landed, so `demo.rs` is unchanged.

## Trust

No protected path is touched. `scripts/exit/s13.sh` and `ci.yml` are unchanged.

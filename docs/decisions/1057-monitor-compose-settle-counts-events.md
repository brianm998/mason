# 1057: Compose settle counted events as reaching the screen

Builds on [1019](1019-monitor-compose-ubuntu-red-conformance-run-45005421555.md) and 0905 (s1026/s1028).

1019 chose evidence only. This decision changes `settle()` only: it tested `lastSeq` (moved by events, Model.kt:1095) where `step()` (Tool.kt:90) and the Rust capturer use view seq. It now tests `lastViewSeq`.

The cause is a hypothesis, unconfirmed. On the next red, read the `settle: quiet exit` and `step: quiet exit` lines from each client in the log: a Compose line with views behind the daemon seq means the capture was early. `step()`, deadlines, compare rules and expected files are unchanged.

## Update: the rule is 1026's signal settle (1093)

A merge dropped the lines this decision added. [1026](1026-compose-conformance-signal-settle.md)'s signal settle is the rule, and it keeps the `lastViewSeq` test: `covered = model.lastViewSeq >= seq`. There is no `step: quiet exit` line, because `step()` calls `settle()`. The Compose diagnostic is the `settle: quiet exit, views at N, daemon seq M` line, printed when `settle()` leaves on the agreed round trips with the views behind. `QUIET_TICKS` is not part of Compose. Under 1026 the daemon may send no view for an unchanged screen, so a views-behind line is evidence, not by itself a fault. See [1093](1093-compose-settle-signal-with-quiet-exit-line.md).

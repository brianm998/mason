# 1057: Compose settle counted events as reaching the screen

Builds on [1019](1019-monitor-compose-ubuntu-red-conformance-run-45005421555.md) and 0905 (s1026/s1028).

1019 chose evidence only. This decision changes `settle()` only: it tested `lastSeq` (moved by events, Model.kt:1095) where `step()` (Tool.kt:90) and the Rust capturer use view seq. It now tests `lastViewSeq`.

The cause is a hypothesis, unconfirmed. On the next red, read the `settle: quiet exit` and `step: quiet exit` lines from each client in the log: a Compose line with views behind the daemon seq means the capture was early. `step()`, deadlines, compare rules and expected files are unchanged.

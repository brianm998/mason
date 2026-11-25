# 1107: one stable bundle for the coordinator's masond

macOS enforces Local Network permission by Mach-O UUID and records a rule per executable path. A new bare path per runtime, on an external disk, left a rule that was allowed and never applied (2026-11-25).

- **One bundle, one path** (`~/Applications/MASON Coordinator.app`), not a bundle per runtime: the rule keeps naming a readable path across updates. `MASON.app` is taken by the monitor.
- **All binaries go in it**, so code that looks beside `masond` needs no change. The replaced bundle is kept for one takeover because running step hosts may use its files.
- **The runtimes do not move.** They are the source of each install, what the self-test runs, and what a rollback installs from. Only what is executed has to be on the internal disk.
- **Opt-in by default only where safe**: a signed coordinator with the default data directory. A scratch daemon must not replace the owner's bundle, an ad-hoc signature would drop privacy grants at each update, and a node's launch daemon is not subject to Local Network.
- **A failure to build the bundle never blocks an update**: the runtime's own `masond` runs, with a warning.
- **The new message needs both a ping answer and an external executable**; the ticket's words name the external disk, so they are not shown when that is not known to be true.

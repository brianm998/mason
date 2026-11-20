# 0719: debt payment locks belong to the daemon and their waits can be cancelled

Context: `suites::pay` blocked for ever in a test binary (76 and 86 minute hangs on 2026-11-20) on a process-wide mutex and an uncancellable lease wait.

Decision: `PAYING` and `IDLE_RUN` are fields of `Daemon`. Both waits in `pay` end on daemon shutdown or when the debt is no longer open, returning `PAY_CANCELLED`, which leaves the debt untried. No timeout was added. `debt_interval = 0` disables the idle pass for tests that pay by hand.

Consequences: "one payment at a time" now holds per daemon, which is per machine for the one masond. Two daemons in one process (tests only) may pay at once. A payment can be cancelled by a concurrent payment that closes its debt.

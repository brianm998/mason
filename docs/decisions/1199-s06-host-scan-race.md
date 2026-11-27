# Decision 1199: tolerate the host.kv race in s06

The s06 fixture reads `host.kv` files that something removes occasionally on Linux. We make the fixture tolerant (skip an unreadable record) rather than add a retry or longer wait, because the check only needs the one live host. The deleter was not identified; finding it needs a follow-up. Daemon and step-host code are unchanged.

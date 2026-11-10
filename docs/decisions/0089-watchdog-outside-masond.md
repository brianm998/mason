# 0089: the watchdog is a bash script run by launchd, not part of masond

Status: accepted

**Context.** `daemon_down` can only be raised by something that is not the daemon. A hung daemon (wedged store writer, full worker pool) stays alive, so `KeepAlive` does not help.

**Decision.** A shell script on a 60-second launchd interval, using only Apple's binaries so it survives a `syspolicyd` outage. It polls `/status`, which exercises the worker pool and the writer. Two consecutive failures define down (one failure may be a restart); three consecutive young or changed-pid runs define crash-looping (one restart is normal). It repeats every 30 minutes and says "back" once. Not named `check-*` or `gen-*` and not protected. Tests drive it with `MASON_WATCHDOG_NOW` rather than sleeping.

**Consequences.** It does not use the daemon's notify routing (quiet hours, dedup): it speaks and pushes every time. Parsing `notify.toml` and JSON in bash is deliberately minimal (`[ntfy]` `server`/`topic`/`token`/`token_env`/`token_file`; no email or webhook).

# 0088: Every alert that stops work is routed; the exceptions are named

Only seven kinds had a route, so the disk-floor alert (true for 2h12m on 2026-11-09) made no sound and sent nothing. Ten more kinds that stop work are now routed like `failed`: monitor, sound and speech at once, push after 10 minutes. `paused`, `debt` and `provider_hold` stay on the monitor alone, listed in `MONITOR_ONLY` beside the routing table: the owner caused the first and last, and a skipped check owed stops nothing.

`ALERT_KINDS` lists every kind the daemon raises, and a core test requires each to be routed or monitor-only, so a new kind cannot fall through silently. The unknown-kind fallback in `Routing::rules` is unchanged (monitor only). Monitor clients migrate a saved `speech.kinds` once with a marker key, rather than comparing lists, so a later untick is respected. No trust or gate file was changed.

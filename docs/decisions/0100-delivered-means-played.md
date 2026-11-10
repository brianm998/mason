# 0100: Sound and speech are delivered when played, not when queued

`tick` recorded sound and speech as delivered the moment it queued the announcement. With no monitor present the announcement expired after ten minutes, the record still said delivered, and neither the other channels' logic nor the reminder ever tried again (20 of 23 spoken `failed` announcements on 2026-11-09).

Decision: the delivery is recorded by whoever takes the announcement (a claiming monitor, or the daemon's speech command, speech only). An expired row records nothing. To avoid a flood after a long absence, a claiming speaker gets one catch-up announcement per live, unacknowledged, never-heard alert, keyed so it happens once; a reminder that fell due in the meantime is folded into it. Announcements about cleared or acknowledged alerts are still dropped by `prune`.

Alternatives rejected: re-queuing every expired announcement (a flood, and old news); recording a separate "expired" delivery (needs a new state in `due`; absence of an ok row already means undelivered). Consequence: a sound whose only taker is the daemon stays undelivered, so `due` re-offers it each tick; the keyed insert makes that a no-op. No gate, guard or trust file changed.

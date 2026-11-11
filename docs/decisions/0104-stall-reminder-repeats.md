# 0104: Repeat the reminder only while the whole pipeline is stopped

One reminder per alert is right for most kinds, but a `stalled` or `disk` alert that lasts all night made two sounds. Those two kinds (`REPEATING` in `crates/core/src/notify.rs`, next to the routing table) now repeat every `[reminder] repeat` (default `2h`, `0` off). Other kinds keep one reminder.

Rounds are laid on a grid from the alert's start, not from the last delivery: this keeps `due` a function of the record and the clock, and a daemon that was off or in quiet hours sends one repeat, not a burst. A repeat is its own announcement key (`reminder<k>`), otherwise the queue would swallow it as a duplicate. Acknowledging, quiet hours and "only channels that delivered the first notice" are unchanged.

No gate, guard, budget, verdict or trust file changed. Not verified: behaviour over real hours; non-audible channels against real receivers.

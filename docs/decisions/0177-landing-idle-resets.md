# 0177: the landing idle reading resets

`landing_idle_ms` is a snapshot of the current idle spell. The counter is cleared on any look at the queue where a batch runs or nothing waits. Rejected: a decaying total (hides how long the current spell is).

# 0327: Sidebar sections put tasks that are not waiting first

Decision: outside Queued, a section sorts by (is_waiting, newest state entry, higher id). A task that is working should not sit under several that wait for a slot. `is_waiting` covers `Waiting`, `Held` and `Paused`, the activities that give a row its `why_waiting` text. Queued keeps its admission order, which already means "what goes next". Not a gate, guard, budget or trust file change.

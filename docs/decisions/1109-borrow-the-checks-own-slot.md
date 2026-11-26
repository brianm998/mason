# 1109: a fallback run borrows the slots its caller holds

## Context

A check bound for a node takes this machine's slots before it is sent. When the node, and then GitHub, give it back, the fallback asked the broker for the same slots again, beside the lease still held (#1149). With a limit of N, N such checks lock the machine.

## Decision

- The lease the caller holds for the run is the run's. The fallback asks only for the shortfall.
- A request made while its caller holds a local slot gets its own band, 1: after landings (0), before everything else (2 and up). It is not a landing, so it is exempt from no gate.
- The deadlock detector counts only holders recorded as kept by a waiting request of their own. A task that holds one suite's slot while a second suite queues is building, and is not reported.
- The alert reuses the kind `stalled`, which repeats until acknowledged, because the queue does not move.

## Not chosen

- Releasing the held slot and queueing again: another task could take it, and the check would wait behind work that started after it.
- Granting the second request over the limit: that weakens the limit.

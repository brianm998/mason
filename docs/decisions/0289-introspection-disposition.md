# 0289: the owner's table decides whether an introspection finding is proposed or queued

Stage s295. Status: proposed.

## Context
Part 14 says the owner's preference decides whether findings are proposed or enqueued, per severity and cost. The code only split filings by who filed and whether the task blocked, so every finding got `agent_normal`. The analyst's output was read only for a task with an exact title.

## Decision
- `[proposals.introspection]` has four keys, `low_small`, `low_large`, `high_small`, `high_large`, each "proposed" or "queued". Any other key or value is a load error. With no table everything is proposed.
- A finding carries an estimated impact (low or high) and cost (small or large). The analyst may state both per finding; absent means high and large, bad values are refused. Detectors are estimated high and large until a rule has a basis to say otherwise.
- The proposal row keeps `severity` as low or high for findings and a new `decided_by` naming the rule that chose the disposition.
- The analyst's task is the one recorded as `last_task` of its schedule row, not the one with a given title.
- Caps, duplicate and evidence rules are not changed, and the table cannot bypass them.

## Not done
No estimate of cost by the detectors. The introspection table replaces `agent_normal` for findings, so a project relying on `agent_normal = "queued"` must set the new keys.

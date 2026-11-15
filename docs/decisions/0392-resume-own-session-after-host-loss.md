# 0392: Resume the agent's own session after a lost host

Status: accepted (task #410, s399).

A lost agent host is continued with `--resume <session id>` when the id is known (decision 0069); only without an id, or with a gone transcript, does a fresh session start. The note now gives the time of the loss. Losses before a resumed session that ended on its own are not counted again toward `MAX_AGENT_RESTARTS` in the same step run. The cap itself, the 4x hard cap on lost invocations and the other counting rules are unchanged. No gate or protected file is touched.
